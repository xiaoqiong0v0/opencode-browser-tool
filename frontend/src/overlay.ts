// 覆盖层 Webview 入口:Canvas 自绘高亮框/批注标记/截图选区 + 鼠标事件上报
// 两种拦截模式(由 Rust setMode 切换):
//   annotate: 点击元素 → 弹批注输入框
//   shot:     双击截全屏 / 拖动框选截区域 → 预览工具栏(说明/取消/保存/发送)
import { createIcons, Check, Send, X } from "lucide";
import { emit } from "@tauri-apps/api/event";
import type { AnnotationMark, HighlightRect, OverlayApi } from "./types";
import { initTheme } from "./theme";

/** 覆盖层模式 */
type OverlayMode = "none" | "annotate" | "shot";

class OverlayRenderer implements OverlayApi {
  private canvas: HTMLCanvasElement;
  private ctx: CanvasRenderingContext2D;
  private lastMove = 0;
  /** 当前模式(Rust 切换) */
  private mode: OverlayMode = "none";
  /** 框选起点 */
  private selStart: { x: number; y: number } | null = null;
  /** 当前截图选区(框选确认后,可拖手柄调整) */
  private sel: { x: number; y: number; w: number; h: number } | null = null;
  /** 正在拖拽的手柄(nw/ne/sw/se),null 未拖拽 */
  private dragHandle: string | null = null;
  /** 手柄拖拽起始 */
  private dragStart: { x: number; y: number } | null = null;
  /** 手柄拖拽前的选区 */
  private dragOrig: { x: number; y: number; w: number; h: number } | null = null;
  /** 预览裁剪选区(以图片显示尺寸为坐标,左上角原点),null 表示整图 */
  private cropSel: { x: number; y: number; w: number; h: number } | null = null;
  /** 预览框选起点 */
  private cropStart: { x: number; y: number } | null = null;
  /** 预览框选中的临时矩形(未确认) */
  private cropTemp: { x: number; y: number; w: number; h: number } | null = null;
  /** 预览手柄拖拽状态 */
  private cropDragHandle: string | null = null;
  private cropDragStart: { x: number; y: number } | null = null;
  private cropDragOrig: { x: number; y: number; w: number; h: number } | null = null;

  constructor() {
    const canvas = document.getElementById("canvas") as HTMLCanvasElement;
    this.canvas = canvas;
    this.ctx = canvas.getContext("2d")!;
    window.addEventListener("resize", () => this.redraw([], []));
    // 批注弹框/截图预览内部点击阻止冒泡,避免关闭瞬间冒泡到 window 触发元素选取
    const notePop = document.getElementById("note-pop") as HTMLDivElement;
    notePop.addEventListener("click", (e) => e.stopPropagation());
    const shotPreview = document.getElementById("shot-preview") as HTMLDivElement;
    shotPreview.addEventListener("click", (e) => {
      e.stopPropagation();
      // 点击遮罩(非预览内容/工具条)自然关闭:有说明文字保存,无则取消,并清空说明
      if (e.target === shotPreview) {
        const input = document.getElementById("shot-note") as HTMLTextAreaElement;
        if (input.value.trim()) {
          void emit("overlay-input", { type: "shot-save", note: input.value, image: this.computeCrop() });
        } else {
          void emit("overlay-input", { type: "shot-cancel" });
        }
        input.value = "";
      }
    });
    this.bindMouse();
    this.bindNotePop();
    this.bindShotPreview();
  }

  /** 设置覆盖层模式(由 Rust 调用) */
  setMode(mode: OverlayMode): void {
    this.mode = mode;
    // 批注模式:鼠标变十字准心(提示可点击选取元素);退出时恢复默认
    document.body.classList.toggle("annotate", mode === "annotate");
    this.selStart = null;
    this.sel = null;
    this.dragHandle = null;
    this.dragStart = null;
    this.dragOrig = null;
  }

  /** 鼠标事件 → Rust(批注/截图模式联动) */
  private bindMouse(): void {
    window.addEventListener("mousemove", (e) => {
      // 截图模式
      if (this.mode === "shot") {
        if (shotPreviewVisible()) return;
        // 拖拽手柄调整选区
        if (this.dragHandle && this.dragStart && this.dragOrig) {
          this.updateSelByHandle(e.clientX - this.dragStart.x, e.clientY - this.dragStart.y);
          return;
        }
        // 框选中实时画选区
        if (this.selStart) {
          this.drawSelection(this.selStart.x, this.selStart.y, e.clientX, e.clientY);
        } else {
          // 无框选时画当前选区(含手柄)
          this.drawSelection(null, 0, 0, 0);
        }
        return;
      }
      // 批注弹框打开:暂停元素选取高亮(外部已半透明遮罩)
      if (notePopVisible()) return;
      const now = performance.now();
      if (now - this.lastMove < 80) return;
      this.lastMove = now;
      void emit("overlay-input", { type: "move", x: e.clientX, y: e.clientY });
    });
    window.addEventListener("mousedown", (e) => {
      if (this.mode === "shot") {
        // 点击预览/工具条内部不触发
        if (shotPreviewVisible()) return;
        // 命中选区手柄 → 开始调整选区大小
        if (this.sel) {
          const hd = this.hitHandle(e.clientX, e.clientY);
          if (hd) {
            this.dragHandle = hd;
            this.dragStart = { x: e.clientX, y: e.clientY };
            this.dragOrig = { ...this.sel };
            e.preventDefault();
            return;
          }
          // 点击选区外部 → 重新框选
          if (e.clientX < this.sel.x || e.clientX > this.sel.x + this.sel.w ||
              e.clientY < this.sel.y || e.clientY > this.sel.y + this.sel.h) {
            this.sel = null;
            this.drawSelection(null, 0, 0, 0);
          }
        }
        this.selStart = { x: e.clientX, y: e.clientY };
        e.preventDefault();
      }
    });
    window.addEventListener("mouseup", (e) => {
      if (this.mode !== "shot") return;
      if (shotPreviewVisible()) return;
      // 结束手柄拖拽
      if (this.dragHandle) {
        this.dragHandle = null;
        this.dragStart = null;
        this.dragOrig = null;
        return;
      }
      // 结束框选:确认选区(暂不截图,双击选区确认)
      if (this.selStart) {
        const sx = Math.min(this.selStart.x, e.clientX);
        const sy = Math.min(this.selStart.y, e.clientY);
        const w = Math.abs(e.clientX - this.selStart.x);
        const h = Math.abs(e.clientY - this.selStart.y);
        this.selStart = null;
        if (w >= 5 && h >= 5) {
          this.sel = { x: sx, y: sy, w, h };
        }
        this.drawSelection(null, 0, 0, 0);
      }
    });
    window.addEventListener("click", (e) => {
      // 点击批注弹框/截图预览内部(含刚关闭的):绝不触发元素选取(双保险,不依赖弹框可见状态)
      const target = e.target as Element | null;
      if (target && target.closest && target.closest("#note-pop, #shot-preview")) return;
      // 批注弹框打开:点击弹框外部 → 关闭弹框且不触发元素选取
      if (notePopVisible()) {
        const pop = document.getElementById("note-pop") as HTMLDivElement;
        if (pop.contains(e.target as Node)) return;
        // 自然关闭(点击外部):有说明文字则保存,无则取消
        const input = document.getElementById("note-pop-input") as HTMLTextAreaElement;
        if (input.value.trim()) {
          void emit("overlay-input", { type: "note-submit", text: input.value });
        } else {
          void emit("overlay-input", { type: "note-cancel" });
        }
        hideNotePop();
        this.hideMask();
        return;
      }
      // 面板遮罩显示时:点击 → 关闭面板(不上报批注 click)
      if (maskVisible()) {
        void emit("overlay-input", { type: "mask-click", x: e.clientX, y: e.clientY });
        return;
      }
      if (this.mode === "annotate") {
        void emit("overlay-input", { type: "click", x: e.clientX, y: e.clientY });
      }
    });
    window.addEventListener("dblclick", (e) => {
      if (this.mode === "shot") {
        if (shotPreviewVisible()) return;
        // 有选区 → 双击选区确认截图;无选区 → 双击截全屏
        if (this.sel) {
          void emit("overlay-input", {
            type: "shot-select",
            x: this.sel.x,
            y: this.sel.y,
            w: this.sel.w,
            h: this.sel.h,
          });
          this.sel = null;
          this.drawSelection(null, 0, 0, 0);
        } else {
          void emit("overlay-input", {
            type: "shot-full",
            w: window.innerWidth,
            h: window.innerHeight,
          });
        }
        e.preventDefault();
      }
    });
    window.addEventListener("contextmenu", (e) => {
      e.preventDefault();
      void emit("overlay-input", { type: "right", x: e.clientX, y: e.clientY });
    });
  }

  /** 显示面板遮罩(面板打开时调用) */
  showMask(): void {
    const mask = document.getElementById("mask") as HTMLDivElement;
    mask.style.display = "block";
  }

  /** 隐藏面板遮罩 */
  hideMask(): void {
    const mask = document.getElementById("mask") as HTMLDivElement;
    mask.style.display = "none";
  }

  /** 批注输入弹框:确定/取消/发送 → emit 给 Rust(按钮级阻止冒泡,避免关闭后 window 误判为元素点击) */
  private bindNotePop(): void {
    // 保存/发送必须有说明文字,空说明不放行
    document.getElementById("note-pop-ok")!.addEventListener("click", (e) => {
      e.stopImmediatePropagation();
      const input = document.getElementById("note-pop-input") as HTMLTextAreaElement;
      if (!input.value.trim()) {
        this.notify("请输入批注说明", "bad");
        return;
      }
      void emit("overlay-input", { type: "note-submit", text: input.value });
      hideNotePop();
      this.hideMask();
    });
    // 保存批注 + 发送所有记录(先保存当前,再发送,发送后清空说明)
    document.getElementById("note-pop-send")!.addEventListener("click", (e) => {
      e.stopImmediatePropagation();
      const input = document.getElementById("note-pop-input") as HTMLTextAreaElement;
      if (!input.value.trim()) {
        this.notify("请输入批注说明", "bad");
        return;
      }
      void emit("overlay-input", { type: "note-send", text: input.value });
      input.value = "";
      hideNotePop();
      this.hideMask();
    });
    document.getElementById("note-pop-cancel")!.addEventListener("click", (e) => {
      e.stopImmediatePropagation();
      void emit("overlay-input", { type: "note-cancel" });
      hideNotePop();
      this.hideMask();
    });
  }

  /** 在坐标处显示批注输入弹框(定位在点击位置右下方,避免超出视口;外部加半透明遮罩提示) */
  showNoteInput(x: number, y: number, selector: string): void {
    const pop = document.getElementById("note-pop") as HTMLDivElement;
    const title = document.getElementById("note-pop-title") as HTMLDivElement;
    const input = document.getElementById("note-pop-input") as HTMLTextAreaElement;
    title.textContent = `添加批注：${selector}`;
    input.value = "";
    pop.style.display = "block";
    // 外部半透明遮罩,提示弹框模式(点击遮罩关闭弹框,不再响应元素选取)
    this.showMask();
    // 先显示再测量尺寸,再定位
    const w = pop.offsetWidth;
    const h = pop.offsetHeight;
    let px = x + 16;
    let py = y + 16;
    if (px + w > window.innerWidth) px = Math.max(0, x - w - 16);
    if (py + h > window.innerHeight) py = Math.max(0, y - h - 16);
    pop.style.left = px + "px";
    pop.style.top = py + "px";
    input.focus();
  }

  /** 截图预览工具栏:取消/保存/发送 → emit 给 Rust(按钮级阻止冒泡) */
  private bindShotPreview(): void {
    document.getElementById("shot-cancel")!.addEventListener("click", (e) => {
      e.stopImmediatePropagation();
      void emit("overlay-input", { type: "shot-cancel" });
    });
    document.getElementById("shot-save")!.addEventListener("click", (e) => {
      e.stopImmediatePropagation();
      const input = document.getElementById("shot-note") as HTMLTextAreaElement;
      if (!input.value.trim()) {
        this.notify("请输入截图说明", "bad");
        return;
      }
      // 有自定义选区按选区裁剪,null 表示整图(默认选中整图)
      const crop = this.computeCrop();
      void emit("overlay-input", { type: "shot-save", note: input.value, image: crop });
      input.value = "";
    });
    document.getElementById("shot-send")!.addEventListener("click", (e) => {
      e.stopImmediatePropagation();
      // 发送:先保存当前截图(需说明),再发送所有,发送后清空说明
      const input = document.getElementById("shot-note") as HTMLTextAreaElement;
      if (!input.value.trim()) {
        this.notify("请输入截图说明", "bad");
        return;
      }
      void emit("overlay-input", { type: "shot-send", note: input.value, image: this.computeCrop() });
      input.value = "";
    });
    // 预览图像滑动框选裁剪区域 / 拖手柄调整
    this.bindCropSelect();
  }

  /** 预览裁剪交互:在图片上滑动框选裁剪区域,四角手柄拖拽调整大小 */
  private bindCropSelect(): void {
    const vp = document.getElementById("shot-preview")!.querySelector(".shot-img-wrap") as HTMLDivElement;

    // 视口坐标 → 图片显示坐标(图片左上角为原点)
    const toImg = (e: MouseEvent): { x: number; y: number } => {
      const r = vp.getBoundingClientRect();
      return { x: e.clientX - r.left, y: e.clientY - r.top };
    };

    vp.addEventListener("mousedown", (e) => {
      if (e.button !== 0 || !shotPreviewVisible()) return;
      e.preventDefault();
      const p = toImg(e);
      // 命中选区四角手柄 → 调整
      if (this.cropSel) {
        const hd = this.hitCropHandle(p.x, p.y);
        if (hd) {
          this.cropDragHandle = hd;
          this.cropDragStart = { x: p.x, y: p.y };
          this.cropDragOrig = { ...this.cropSel };
          return;
        }
        // 点击选区内:忽略(避免误清);选区外:重新框选
        if (p.x >= this.cropSel.x && p.x <= this.cropSel.x + this.cropSel.w &&
            p.y >= this.cropSel.y && p.y <= this.cropSel.y + this.cropSel.h) {
          return;
        }
        this.cropSel = null;
      }
      this.cropStart = p;
      this.cropTemp = null;
    });
    vp.addEventListener("mousemove", (e) => {
      if (!shotPreviewVisible()) return;
      const p = toImg(e);
      // 拖手柄调整选区
      if (this.cropDragHandle && this.cropDragStart && this.cropDragOrig) {
        this.updateCropByHandle(p.x - this.cropDragStart.x, p.y - this.cropDragStart.y);
        return;
      }
      // 框选中:实时绘制临时选区(存入 cropTemp,松开确认)
      if (this.cropStart) {
        const sx = Math.min(this.cropStart.x, p.x);
        const sy = Math.min(this.cropStart.y, p.y);
        const w = Math.abs(p.x - this.cropStart.x);
        const h = Math.abs(p.y - this.cropStart.y);
        const tmp = { x: sx, y: sy, w, h };
        this.cropTemp = tmp;
        this.renderCropBox(tmp);
      }
    });
    const done = () => {
      // 框选完成:临时选区确认(最小 20px)
      if (this.cropTemp && this.cropTemp.w >= 20 && this.cropTemp.h >= 20) {
        this.cropSel = this.cropTemp;
      }
      this.cropTemp = null;
      this.cropStart = null;
      // 手柄拖拽结束
      this.cropDragHandle = null;
      this.cropDragStart = null;
      this.cropDragOrig = null;
      this.renderCropBoxFromSel();
    };
    window.addEventListener("mouseup", done);
    window.addEventListener("mouseleave", done);
    // 双击恢复选中整图
    vp.addEventListener("dblclick", (e) => {
      if (!shotPreviewVisible()) return;
      e.preventDefault();
      this.selectFullImage();
    });
  }

  /** 命中预览选区四角手柄(nw/ne/sw/se) */
  private hitCropHandle(x: number, y: number): string | null {
    if (!this.cropSel) return null;
    const s = this.cropSel;
    const R = 12;
    const corners: [string, number, number][] = [
      ["nw", s.x, s.y],
      ["ne", s.x + s.w, s.y],
      ["sw", s.x, s.y + s.h],
      ["se", s.x + s.w, s.y + s.h],
    ];
    for (const [name, hx, hy] of corners) {
      if (Math.abs(x - hx) <= R && Math.abs(y - hy) <= R) return name;
    }
    return null;
  }

  /** 根据手柄拖拽更新裁剪选区(相对图片显示坐标) */
  private updateCropByHandle(dx: number, dy: number): void {
    if (!this.cropSel || !this.cropDragOrig || !this.cropDragHandle) return;
    const o = this.cropDragOrig;
    const MIN = 20;
    let { x, y, w, h } = o;
    if (this.cropDragHandle.includes("w")) {
      x = Math.max(0, Math.min(o.x + dx, o.x + o.w - MIN));
      w = o.w + (o.x - x);
    }
    if (this.cropDragHandle.includes("e")) {
      w = Math.max(MIN, o.w + dx);
    }
    if (this.cropDragHandle.includes("n")) {
      y = Math.max(0, Math.min(o.y + dy, o.y + o.h - MIN));
      h = o.h + (o.y - y);
    }
    if (this.cropDragHandle.includes("s")) {
      h = Math.max(MIN, o.h + dy);
    }
    this.cropSel = { x, y, w, h };
    this.renderCropBox(this.cropSel);
  }

  /** 按当前 cropSel 绘制选区框(cropSel=null 表示整图,框覆盖整个图片) */
  private renderCropBoxFromSel(): void {
    this.renderCropBox(this.cropSel);
  }

  /** 绘制裁剪选区框到图片上;sel=null 表示选中整图,框覆盖整个图片显示区域 */
  private renderCropBox(sel: { x: number; y: number; w: number; h: number } | null): void {
    const box = document.getElementById("crop-box") as HTMLDivElement;
    const img = document.getElementById("shot-img") as HTMLImageElement;
    box.classList.add("crop-active");
    const l = sel ? sel.x : 0;
    const t = sel ? sel.y : 0;
    const w = sel ? sel.w : img.offsetWidth;
    const h = sel ? sel.h : img.offsetHeight;
    box.style.left = l + "px";
    box.style.top = t + "px";
    box.style.width = w + "px";
    box.style.height = h + "px";
  }

  /** 选中整图(默认态/双击恢复):cropSel=null 表示整图同时移除拖拽状态 */
  private selectFullImage(): void {
    this.cropSel = null;
    this.cropStart = null;
    this.cropTemp = null;
    this.cropDragHandle = null;
    this.cropDragStart = null;
    this.cropDragOrig = null;
    this.renderCropBox(null);
  }

  /** 计算裁剪选区对应的图片 base64;无选区返回 null(表示保存整图) */
  private computeCrop(): string | null {
    if (!this.cropSel) return null;
    const img = document.getElementById("shot-img") as HTMLImageElement;
    if (!img.complete || img.naturalWidth === 0) return null;
    const sel = this.cropSel;
    const bw = img.offsetWidth; // 显示尺寸
    const bh = img.offsetHeight;
    const nw = img.naturalWidth; // 原图像素
    const nh = img.naturalHeight;
    // clamp 选区到图片显示边界内
    let cx = Math.max(0, Math.min(sel.x, bw));
    let cy = Math.max(0, Math.min(sel.y, bh));
    let cw = Math.max(1, Math.min(sel.w, bw - cx));
    let ch = Math.max(1, Math.min(sel.h, bh - cy));
    // 换算到原图像素坐标并裁剪
    const kx = nw / bw;
    const ky = nh / bh;
    const canvas = document.createElement("canvas");
    canvas.width = Math.max(1, Math.round(cw * kx));
    canvas.height = Math.max(1, Math.round(ch * ky));
    const ctx = canvas.getContext("2d")!;
    ctx.drawImage(img, cx * kx, cy * ky, cw * kx, ch * ky, 0, 0, canvas.width, canvas.height);
    return canvas.toDataURL("image/png").replace("data:image/png;base64,", "");
  }

  /** 显示截图预览(全屏 + 底部工具栏),默认选中整图 */
  showShotPreview(dataUrl: string): void {
    const box = document.getElementById("shot-preview") as HTMLDivElement;
    const img = document.getElementById("shot-img") as HTMLImageElement;
    img.src = dataUrl;
    img.onload = () => this.selectFullImage();
    this.selectFullImage();
    // 每次打开清空上一条说明
    (document.getElementById("shot-note") as HTMLTextAreaElement).value = "";
    // 清空全屏选区与拖拽状态(截图模式选区)
    this.sel = null;
    this.dragHandle = null;
    this.dragStart = null;
    this.dragOrig = null;
    box.classList.add("show");
  }

  /** 隐藏批注弹框(退出批注模式/关闭面板时调用) */
  hideNotePop(): void {
    (document.getElementById("note-pop-input") as HTMLTextAreaElement).value = "";
    hideNotePop();
    this.hideMask();
  }

  /** 隐藏截图预览 */
  hideShotPreview(): void {
    const box = document.getElementById("shot-preview") as HTMLDivElement;
    box.classList.remove("show");
  }

  /** 画截图选区框(框选实时反馈),传 null 清除;已确认选区额外画四角手柄 */
  private drawSelection(x: number | null, y: number, w: number, h: number): void {
    const rects: HighlightRect[] = [];
    const marks: AnnotationMark[] = this._marks || [];
    // 已确认选区:画选区 + 手柄
    if (this.sel) {
      const s = this.sel;
      rects.push({ x: s.x, y: s.y, w: s.w, h: s.h, color: 0x2196f3 });
      this.redraw(rects, marks);
      this.drawHandles(s.x, s.y, s.w, s.h);
      return;
    }
    // 正在框选:画临时框
    if (x !== null) {
      const sx = Math.min(x, w);
      const sy = Math.min(y, h);
      const sw = Math.abs(w - x);
      const sh = Math.abs(h - y);
      rects.push({ x: sx, y: sy, w: sw, h: sh, color: 0x2196f3 });
    }
    this.redraw(rects, marks);
  }

  /** 画选区四角调整手柄 */
  private drawHandles(x: number, y: number, w: number, h: number): void {
    const ctx = this.ctx;
    const dpr = window.devicePixelRatio || 1;
    ctx.save();
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    const corners: [number, number][] = [
      [x, y],
      [x + w, y],
      [x, y + h],
      [x + w, y + h],
    ];
    for (const [hx, hy] of corners) {
      ctx.fillStyle = "rgba(33,150,243,0.95)";
      ctx.fillRect(hx - 5, hy - 5, 10, 10);
      ctx.strokeStyle = "#ffffff";
      ctx.lineWidth = 1.5;
      ctx.strokeRect(hx - 5, hy - 5, 10, 10);
    }
    ctx.restore();
  }

  /** 命中选区四角手柄(nw/ne/sw/se),未命中返回 null */
  private hitHandle(x: number, y: number): string | null {
    if (!this.sel) return null;
    const s = this.sel;
    const R = 10;
    const corners: [string, number, number][] = [
      ["nw", s.x, s.y],
      ["ne", s.x + s.w, s.y],
      ["sw", s.x, s.y + s.h],
      ["se", s.x + s.w, s.y + s.h],
    ];
    for (const [name, hx, hy] of corners) {
      if (Math.abs(x - hx) <= R && Math.abs(y - hy) <= R) return name;
    }
    return null;
  }

  /** 根据手柄拖拽偏移更新选区(保持最小尺寸) */
  private updateSelByHandle(dx: number, dy: number): void {
    if (!this.sel || !this.dragOrig || !this.dragHandle) return;
    const o = this.dragOrig;
    const MIN = 10;
    let { x, y, w, h } = o;
    const hd = this.dragHandle;
    if (hd.includes("w")) {
      const nx = Math.min(o.x + dx, o.x + o.w - MIN);
      x = nx;
      w = o.w + (o.x - nx);
    }
    if (hd.includes("e")) {
      w = Math.max(MIN, o.w + dx);
    }
    if (hd.includes("n")) {
      const ny = Math.min(o.y + dy, o.y + o.h - MIN);
      y = ny;
      h = o.h + (o.y - ny);
    }
    if (hd.includes("s")) {
      h = Math.max(MIN, o.h + dy);
    }
    this.sel = { x, y, w, h };
    this.drawSelection(null, 0, 0, 0);
  }

  /** 重绘:清空 + 高亮 + 批注标记 */
  redraw(rects: HighlightRect[], marks: AnnotationMark[]): void {
    const dpr = window.devicePixelRatio || 1;
    this.canvas.width = window.innerWidth * dpr;
    this.canvas.height = window.innerHeight * dpr;
    const ctx = this.ctx;
    ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    ctx.clearRect(0, 0, window.innerWidth, window.innerHeight);

    for (const r of rects) {
      ctx.fillStyle = toRgba(r.color, 0.25);
      ctx.fillRect(r.x, r.y, r.w, r.h);
      ctx.strokeStyle = toRgba(r.color, 0.9);
      ctx.lineWidth = 2;
      ctx.strokeRect(r.x, r.y, r.w, r.h);
    }
    for (const m of marks) {
      // 圆形徽章标记:圆底 + 白字编号 + 白描边 + 投影
      const cx = m.x + m.w / 2;
      const cy = m.y + m.h / 2;
      const r = 11;
      ctx.save();
      ctx.shadowColor = "rgba(0,0,0,0.45)";
      ctx.shadowBlur = 4;
      ctx.shadowOffsetY = 1;
      ctx.beginPath();
      ctx.arc(cx, cy, r, 0, Math.PI * 2);
      ctx.fillStyle = toRgba(m.color, 0.97);
      ctx.fill();
      ctx.restore();
      ctx.beginPath();
      ctx.arc(cx, cy, r, 0, Math.PI * 2);
      ctx.strokeStyle = "rgba(255,255,255,0.92)";
      ctx.lineWidth = 2;
      ctx.stroke();
      ctx.fillStyle = "#ffffff";
      ctx.font = "600 12px system-ui, sans-serif";
      ctx.textAlign = "center";
      ctx.textBaseline = "middle";
      ctx.fillText(String(m.index), cx, cy + 0.5);
    }
  }

  /** 显示通知气泡(3 秒后消失),type: ok|bad|err */
  notify(message: string, type?: string): void {
    const el = document.getElementById("toast") as HTMLDivElement;
    el.textContent = message;
    el.className = "show " + (["ok", "bad", "err"].includes(type || "") ? type : "ok");
    clearTimeout((el as unknown as { _t?: number })._t);
    (el as unknown as { _t?: number })._t = window.setTimeout(() => {
      el.className = "";
    }, 3000);
  }

  /** 批注/截图标记(Rust 写入) */
  _marks: AnnotationMark[] = [];
}

/** 批注弹框是否可见(用于鼠标点击判断) */
function notePopVisible(): boolean {
  const pop = document.getElementById("note-pop") as HTMLDivElement;
  return pop.style.display === "block";
}

/** 截图预览是否可见 */
function shotPreviewVisible(): boolean {
  const box = document.getElementById("shot-preview") as HTMLDivElement;
  return box.classList.contains("show");
}

/** 面板遮罩是否可见 */
function maskVisible(): boolean {
  const mask = document.getElementById("mask") as HTMLDivElement;
  return mask.style.display === "block";
}

function hideNotePop(): void {
  const pop = document.getElementById("note-pop") as HTMLDivElement;
  pop.style.display = "none";
}

/** 颜色 u32(0xRRGGBB) → rgba 字符串 */
function toRgba(color: number, alpha: number): string {
  const r = (color >> 16) & 0xff;
  const g = (color >> 8) & 0xff;
  const b = color & 0xff;
  return `rgba(${r},${g},${b},${alpha})`;
}

// 挂载到 window:Rust eval 调用
const api = new OverlayRenderer();
(window as unknown as { __btOverlay: OverlayApi }).__btOverlay = api;
// lucide 图标替换(批注弹框 ✕/✓、截图工具栏 ✕/✓/发送),key 需用 PascalCase
createIcons({ icons: { X, Check, Send } });
void initTheme();
console.log("[overlay] ready");
