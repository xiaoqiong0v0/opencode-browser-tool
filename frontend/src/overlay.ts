// 覆盖层 Webview 入口:Canvas 自绘高亮框/批注标记 + 鼠标事件上报
// 批注模式下覆盖层显示并拦截鼠标(坐标 emit 给 Rust 做元素查询)
import { emit } from "@tauri-apps/api/event";
import type { AnnotationMark, HighlightRect, OverlayApi } from "./types";

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
      void emit("overlay-input", { type: "click", x: e.clientX, y: e.clientY });
    });
    window.addEventListener("contextmenu", (e) => {
      e.preventDefault();
      void emit("overlay-input", { type: "right", x: e.clientX, y: e.clientY });
    });
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
console.log("[overlay] ready");
