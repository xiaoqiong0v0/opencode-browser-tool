// 覆盖层 Webview 入口:Canvas 自绘高亮框/批注标记 + 鼠标事件上报
// 批注模式下覆盖层显示并拦截鼠标(坐标 emit 给 Rust 做元素查询)
import { emit } from "@tauri-apps/api/event";
import type { AnnotationMark, HighlightRect, OverlayApi } from "./types";
import { initTheme } from "./theme";

class OverlayRenderer implements OverlayApi {
  private canvas: HTMLCanvasElement;
  private ctx: CanvasRenderingContext2D;
  private lastMove = 0;

  constructor() {
    const canvas = document.getElementById("canvas") as HTMLCanvasElement;
    this.canvas = canvas;
    this.ctx = canvas.getContext("2d")!;
    window.addEventListener("resize", () => this.redraw([], []));
    this.bindMouse();
    this.bindNotePop();
  }

  /** 鼠标事件 → Rust(批注模式联动) */
  private bindMouse(): void {
    window.addEventListener("mousemove", (e) => {
      // 节流:80ms 内最多上报一次
      const now = performance.now();
      if (now - this.lastMove < 80) return;
      this.lastMove = now;
      void emit("overlay-input", { type: "move", x: e.clientX, y: e.clientY });
    });
    window.addEventListener("click", (e) => {
      // 点击批注弹框内部时不上报
      if (notePopVisible()) return;
      // 面板遮罩显示时:点击 → 关闭面板(不上报批注 click)
      if (maskVisible()) {
        void emit("overlay-input", { type: "mask-click", x: e.clientX, y: e.clientY });
        return;
      }
      void emit("overlay-input", { type: "click", x: e.clientX, y: e.clientY });
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
}

/** 批注弹框是否可见(用于鼠标点击判断) */
function notePopVisible(): boolean {
  const pop = document.getElementById("note-pop") as HTMLDivElement;
  return pop.style.display === "block";
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
void initTheme();
console.log("[overlay] ready");
