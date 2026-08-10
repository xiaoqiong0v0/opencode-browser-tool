import { s } from "./config";
import { post } from "./utils";
import { togglePanel, notify, updateToolbar, sendAll } from "./core";
import type { Dom } from "./handlers";

let d: Dom;
let shotImg: HTMLImageElement | null = null;
let isSel = false,
  selSX = 0,
  selSY = 0,
  curSel = { x: 0, y: 0, w: 0, h: 0 };
let drag: "" | "sel" = "",
  dragOX = 0,
  dragOY = 0,
  dragOS = { x: 0, y: 0, w: 0, h: 0 },
  dragDir = "";
let shotHasSel = false;

function sx(e: MouseEvent): number {
  const c = getCanvas();
  return c ? (e.clientX - c.getBoundingClientRect().left) * (c.width / c.clientWidth) : e.offsetX;
}
function sy(e: MouseEvent): number {
  const c = getCanvas();
  return c ? (e.clientY - c.getBoundingClientRect().top) * (c.height / c.clientHeight) : e.offsetY;
}
function getCanvas(): HTMLCanvasElement | null {
  return d.shotOverlay.querySelector("canvas");
}

export function initShot(doms: Dom) {
  d = doms;
}

export async function activateShot() {
  togglePanel(false, true);
  updateToolbar();
  d.btn.classList.add("hidden");
  d.shotOverlay.innerHTML = `<div class="pw-mp-shot-hint" style="position:fixed;top:50%;left:50%;transform:translate(-50%,-50%);color:rgba(255,255,255,0.5);font-size:14px;z-index:5;pointer-events:none">${s("panel.screenshot_hint")}</div>`;
  d.shotOverlay.classList.add("active");
  const resp = await post("/screenshot", {});
  if (!resp.base64) {
    notify(s("panel.screenshot_failed"), "err");
    deactivateShot();
    return;
  }
  shotImg = new Image();
  shotImg.onload = () => {
    d.shotOverlay.innerHTML = "";
    const c = document.createElement("canvas");
    c.width = window.innerWidth;
    c.height = window.innerHeight;
    c.style.cssText = `width:100vw;height:100vh;display:block`;
    d.shotOverlay.appendChild(c);
    c.getContext("2d")!.drawImage(shotImg!, 0, 0, c.width, c.height);
    // 全屏半透明遮罩
    const ctx = c.getContext("2d")!;
    ctx.fillStyle = "rgba(0,0,0,0.3)";
    ctx.fillRect(0, 0, c.width, c.height);
    // 提示文字直接绘制在 canvas 上
    ctx.fillStyle = "rgba(255,255,255,0.25)";
    ctx.font = "14px sans-serif";
    ctx.textAlign = "center";
    ctx.fillText(s("panel.screenshot_hint"), c.width / 2, c.height / 2);
    const bar = document.createElement("div");
    bar.className = "pw-mp-shot-bar";
    bar.style.display = "none";
    bar.innerHTML = `<input class="pw-mp-shot-desc" placeholder="${s("panel.screenshot_note")}"><button class="pw-mp-shot-ok">✓</button><button class="pw-mp-shot-cancel">✕</button><button class="pw-mp-shot-send">✈</button>`;
    d.shotOverlay.appendChild(bar);
    isSel = false;
    shotHasSel = false;
    curSel = { x: 0, y: 0, w: 0, h: 0 };
    const rh = () => deactivateShot();
    window.addEventListener("resize", rh);
    d.shotOverlay.addEventListener("mousedown", shotMouseDown);
    d.shotOverlay.addEventListener("mousemove", shotMouseMove);
    d.shotOverlay.addEventListener("mouseup", shotMouseUp);
    d.shotOverlay.addEventListener("dblclick", shotDblClick);
    bar.querySelector(".pw-mp-shot-ok")!.addEventListener("click", shotConfirm);
    bar.querySelector(".pw-mp-shot-send")!.addEventListener("click", async () => {
      const desc = (d.shotOverlay.querySelector(".pw-mp-shot-desc") as HTMLInputElement)?.value?.trim();
      if (!desc) return;
      saveScreenshot(desc);
      await sendAll();
      deactivateShot();
    });
    bar.querySelector(".pw-mp-shot-cancel")!.addEventListener("click", () => {
      window.removeEventListener("resize", rh);
      deactivateShot();
    });
    const descInput = bar.querySelector(".pw-mp-shot-desc") as HTMLInputElement;
    const okBtn = bar.querySelector(".pw-mp-shot-ok") as HTMLButtonElement;
    const sendBtn = bar.querySelector(".pw-mp-shot-send") as HTMLButtonElement;
    okBtn.disabled = true;
    sendBtn.disabled = true;
    descInput.addEventListener("input", () => {
      const hasText = descInput.value.trim().length > 0;
      okBtn.disabled = !hasText;
      sendBtn.disabled = !hasText;
    });
  };
  shotImg.src = `data:image/png;base64,${resp.base64}`;
}

function deactivateShot() {
  d.btn.classList.remove("hidden");
  d.shotOverlay.classList.remove("active");
  d.shotOverlay.innerHTML = "";
  shotImg = null;
  shotHasSel = false;
}

function renderCanvas() {
  const c = getCanvas();
  if (!c) return;
  const ctx = c.getContext("2d")!;
  ctx.clearRect(0, 0, c.width, c.height);
  if (shotImg) ctx.drawImage(shotImg, 0, 0, c.width, c.height);
  ctx.fillStyle = "rgba(0,0,0,0.3)";
  ctx.fillRect(0, 0, c.width, c.height);
  if (curSel.w >= 5 && curSel.h >= 5) {
    ctx.globalCompositeOperation = "destination-out";
    ctx.clearRect(curSel.x, curSel.y, curSel.w, curSel.h);
    ctx.globalCompositeOperation = "source-over";
    ctx.strokeStyle = "#00b4ff";
    ctx.lineWidth = 2;
    ctx.strokeRect(curSel.x + 0.5, curSel.y + 0.5, curSel.w - 1, curSel.h - 1);
  } else {
    ctx.fillStyle = "rgba(255,255,255,0.25)";
    ctx.font = "14px sans-serif";
    ctx.textAlign = "center";
    ctx.fillText(s("panel.screenshot_hint"), c.width / 2, c.height / 2);
  }
  updateHandles();
}

function clearHandles() {
  d.shotOverlay.querySelectorAll(".pw-mp-shot-hnd").forEach((e) => e.remove());
}

function updateHandles() {
  clearHandles();
  if (curSel.w < 5 || curSel.h < 5) return;
  const s = curSel,
    dirs = ["nw", "n", "ne", "e", "se", "s", "sw", "w"];
  const pos: Record<string, [number, number]> = {
    nw: [s.x, s.y],
    n: [s.x + s.w / 2, s.y],
    ne: [s.x + s.w, s.y],
    e: [s.x + s.w, s.y + s.h / 2],
    se: [s.x + s.w, s.y + s.h],
    s: [s.x + s.w / 2, s.y + s.h],
    sw: [s.x, s.y + s.h],
    w: [s.x, s.y + s.h / 2],
  };
  for (const d of dirs) {
    const el = document.createElement("div");
    el.className = "pw-mp-shot-hnd " + d;
    el.style.cssText = `left:${pos[d][0] - 4}px;top:${pos[d][1] - 4}px;position:absolute;width:8px;height:8px;background:#00b4ff;border:1px solid #fff;z-index:4;cursor:${d}-resize`;
    d.shotOverlay.appendChild(el);
  }
}

function showBar() {
  const bar = d.shotOverlay.querySelector(".pw-mp-shot-bar") as HTMLElement;
  if (!bar) return;
  bar.style.display = "flex";
  const by = curSel.y + curSel.h + 10;
  bar.style.top = (by + 50 > window.innerHeight ? Math.max(10, curSel.y - 50) : by) + "px";
  // 水平跟随选框中心
  const cx = curSel.x + curSel.w / 2;
  const barW = 320;
  bar.style.left = Math.max(10, Math.min(cx - barW / 2, window.innerWidth - barW - 10)) + "px";
  bar.style.transform = "none";
}

function shotMouseDown(e: MouseEvent) {
  const hnd = (e.target as HTMLElement).closest(".pw-mp-shot-hnd") as HTMLElement;
  if (hnd && shotHasSel) {
    drag = "sel";
    dragDir = hnd.className.split(" ")[1];
    dragOS = { ...curSel };
    dragOX = sx(e);
    dragOY = sy(e);
    return;
  }
  if ((e.target as HTMLElement).closest(".pw-mp-shot-bar, .pw-mp-shot-desc")) return;
  isSel = true;
  drag = "sel";
  dragDir = "";
  selSX = sx(e);
  selSY = sy(e);
  curSel = { x: selSX, y: selSY, w: 0, h: 0 };
  renderCanvas();
}

function shotMouseMove(e: MouseEvent) {
  if (drag !== "sel") return;
  if (isSel && !dragDir) {
    curSel.x = Math.min(selSX, sx(e));
    curSel.y = Math.min(selSY, sy(e));
    curSel.w = Math.abs(sx(e) - selSX);
    curSel.h = Math.abs(sy(e) - selSY);
  } else if (dragDir) {
    const dx = sx(e) - dragOX,
      dy = sy(e) - dragOY,
      o = dragOS;
    let nx = o.x,
      ny = o.y,
      nw = o.w,
      nh = o.h;
    if (dragDir.includes("w")) {
      nx = o.x + dx;
      nw = o.w - dx;
    }
    if (dragDir.includes("e")) {
      nw = o.w + dx;
    }
    if (dragDir.includes("n")) {
      ny = o.y + dy;
      nh = o.h - dy;
    }
    if (dragDir.includes("s")) {
      nh = o.h + dy;
    }
    curSel = { x: nx, y: ny, w: Math.max(20, nw), h: Math.max(20, nh) };
  }
  renderCanvas();
}

function shotMouseUp() {
  if (drag === "sel" && isSel) {
    isSel = false;
    if (curSel.w >= 5 && curSel.h >= 5) {
      shotHasSel = true;
      showBar();
    } else {
      curSel = { x: 0, y: 0, w: 0, h: 0 };
      shotHasSel = false;
      clearHandles();
      const bar = d.shotOverlay.querySelector(".pw-mp-shot-bar") as HTMLElement;
      if (bar) bar.style.display = "none";
    }
  }
  drag = "";
  dragDir = "";
  renderCanvas();
}

function shotDblClick() {
  const c = getCanvas();
  if (!c) return;
  curSel = { x: 0, y: 0, w: c.width, h: c.height };
  isSel = false;
  drag = "";
  shotHasSel = true;
  showBar();
  renderCanvas();
}

function saveScreenshot(desc: string) {
  const c = getCanvas();
  if (!c) return;
  const tmp = document.createElement("canvas");
  tmp.width = curSel.w;
  tmp.height = curSel.h;
  const tctx = tmp.getContext("2d")!;
  tctx.drawImage(shotImg!, curSel.x, curSel.y, curSel.w, curSel.h, 0, 0, curSel.w, curSel.h);
  const fullBase64 = tmp.toDataURL("image/png").split(",")[1];
  let thumb = fullBase64;
  if (curSel.w > 200 || curSel.h > 200) {
    const ratio = Math.min(200 / curSel.w, 200 / curSel.h);
    const tw = Math.round(curSel.w * ratio),
      th = Math.round(curSel.h * ratio);
    const tc = document.createElement("canvas");
    tc.width = tw;
    tc.height = th;
    tc.getContext("2d")!.drawImage(tmp, 0, 0, tw, th);
    thumb = tc.toDataURL("image/png").split(",")[1];
  }
  post("/screenshot", { store: true, pageUrl: window.location.href, note: desc, base64: thumb, fullBase64 });
}

function shotConfirm() {
  const desc = (d.shotOverlay.querySelector(".pw-mp-shot-desc") as HTMLInputElement)?.value?.trim();
  if (!desc) return;
  saveScreenshot(desc);
  notify(s("panel.screenshot_stored"), "ok");
  deactivateShot();
}
