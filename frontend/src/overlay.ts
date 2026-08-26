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

  constructor() {
    const canvas = document.getElementById("canvas") as HTMLCanvasElement;
    this.canvas = canvas;
    this.ctx = canvas.getContext("2d")!;
    window.addEventListener("resize", () => this.redraw([], []));
    this.bindMouse();
    this.bindNotePop();
    this.bindShotPreview();
  }

  /** 设置覆盖层模式(由 Rust 调用) */
  setMode(mode: OverlayMode): void {
    this.mode = mode;
    this.selStart = null;
  }

  /** 鼠标事件 → Rust(批注/截图模式联动) */
  private bindMouse(): void {
    window.addEventListener("mousemove", (e) => {
      // 截图模式:框选中实时画选区,未框选时只上报移动(节流)
      if (this.mode === "shot") {
        if (this.selStart) {
          this.drawSelection(this.selStart.x, this.selStart.y, e.clientX, e.clientY);
        }
        return;
      }
      const now = performance.now();
      if (now - this.lastMove < 80) return;
      this.lastMove = now;
      void emit("overlay-input", { type: "move", x: e.clientX, y: e.clientY });
    });
    window.addEventListener("mousedown", (e) => {
      if (this.mode === "shot") {
        // 点击预览/工具条内部不触发框选
        if (shotPreviewVisible()) return;
        this.selStart = { x: e.clientX, y: e.clientY };
        e.preventDefault();
      }
    });
    window.addEventListener("mouseup", (e) => {
      if (this.mode === "shot" && this.selStart) {
        const sx = Math.min(this.selStart.x, e.clientX);
        const sy = Math.min(this.selStart.y, e.clientY);
        const w = Math.abs(e.clientX - this.selStart.x);
        const h = Math.abs(e.clientY - this.selStart.y);
        this.selStart = null;
        this.drawSelection(null, 0, 0, 0, 0);
        if (w >= 5 && h >= 5) {
          void emit("overlay-input", { type: "shot-select", x: sx, y: sy, w, h });
        }
      }
    });
    window.addEventListener("click", (e) => {
      // 点击批注弹框/截图预览内部时不上报
      if (notePopVisible() || shotPreviewVisible()) return;
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
        // 双击截全屏(上报 viewport 尺寸,配合截整个视口)
        void emit("overlay-input", {
          type: "shot-full",
          w: window.innerWidth,
          h: window.innerHeight,
        });
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

  /** 批注输入弹框:确定/取消 → emit 给 Rust */
  private bindNotePop(): void {
    document.getElementById("note-pop-ok")!.addEventListener("click", () => {
      const input = document.getElementById("note-pop-input") as HTMLTextAreaElement;
      void emit("overlay-input", { type: "note-submit", text: input.value });
      hideNotePop();
    });
    document.getElementById("note-pop-cancel")!.addEventListener("click", () => {
      void emit("overlay-input", { type: "note-cancel" });
      hideNotePop();
    });
  }

  /** 在坐标处显示批注输入弹框(定位在点击位置右下方,避免超出视口) */
  showNoteInput(x: number, y: number, selector: string): void {
    const pop = document.getElementById("note-pop") as HTMLDivElement;
    const title = document.getElementById("note-pop-title") as HTMLDivElement;
    const input = document.getElementById("note-pop-input") as HTMLTextAreaElement;
    title.textContent = `添加批注：${selector}`;
    input.value = "";
    pop.style.display = "block";
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

  /** 截图预览工具栏:取消/保存/发送 → emit 给 Rust */
  private bindShotPreview(): void {
    document.getElementById("shot-cancel")!.addEventListener("click", () => {
      void emit("overlay-input", { type: "shot-cancel" });
    });
    document.getElementById("shot-save")!.addEventListener("click", () => {
      const input = document.getElementById("shot-note") as HTMLTextAreaElement;
      void emit("overlay-input", { type: "shot-save", note: input.value });
      input.value = "";
    });
    document.getElementById("shot-send")!.addEventListener("click", () => {
      void emit("overlay-input", { type: "shot-send" });
    });
  }

  /** 显示截图预览(全屏 + 底部工具栏) */
  showShotPreview(dataUrl: string): void {
    const box = document.getElementById("shot-preview") as HTMLDivElement;
    const img = document.getElementById("shot-img") as HTMLImageElement;
    img.src = dataUrl;
    box.classList.add("show");
  }

  /** 隐藏截图预览 */
  hideShotPreview(): void {
    const box = document.getElementById("shot-preview") as HTMLDivElement;
    box.classList.remove("show");
  }

  /** 画截图选区框(框选实时反馈),传 null 清除 */
  private drawSelection(x: number | null, y: number, w: number, h: number): void {
    const rects: HighlightRect[] = [];
    const marks: AnnotationMark[] = this._marks || [];
    if (x !== null) {
      const sx = Math.min(x, w);
      const sy = Math.min(y, h);
      const sw = Math.abs(w - x);
      const sh = Math.abs(h - y);
      rects.push({ x: sx, y: sy, w: sw, h: sh, color: 0x2196f3 });
    }
    this.redraw(rects, marks);
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
      ctx.fillStyle = toRgba(m.color, 0.95);
      ctx.fillRect(m.x, m.y, m.w, m.h);
      ctx.fillStyle = "#ffffff";
      ctx.font = "14px system-ui, sans-serif";
      ctx.textBaseline = "middle";
      ctx.fillText(String(m.index), m.x + 4, m.y + m.h / 2 + 1);
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
