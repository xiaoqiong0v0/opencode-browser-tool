import * as state from "./state";
import { finishAnnotation, togglePanel, updateToolbar, notify, closeEditBox, loadRecords, initCore, sendAll } from "./core";
import { pickElement, initAnnotate } from "./annotate";
import { activateShot, initShot } from "./shot";

export interface Dom {
  btn: HTMLButtonElement;
  overlay: HTMLElement;
  toolbarEl: HTMLElement;
  urlEl: HTMLElement;
  listEl: HTMLElement;
  cancelBtn: HTMLButtonElement;
  sendBtn: HTMLButtonElement;
  hlEl: HTMLElement;
  hlLabel: HTMLElement;
  editBox: HTMLElement;
  editTextarea: HTMLTextAreaElement;
  editSendBtn: HTMLButtonElement;
  editCancelBtn: HTMLButtonElement;
  editConfirmBtn: HTMLButtonElement;
  editDelBtn: HTMLButtonElement;
  notifyEl: HTMLDivElement;
  closeBtn: HTMLButtonElement;
  shotOverlay: HTMLElement;
}

let d: Dom;

function isPanel(e: Event): boolean {
  return (e as any).composedPath?.().some((el: any) => el?.id === "pw-mp-root" || el?.classList?.contains?.("pw-mp-overlay") || el?.classList?.contains?.("pw-mp-btn") || el?.classList?.contains?.("pw-mp-notify") || el?.classList?.contains?.("pw-mp-editbox"));
}

export function init(doms: Dom) {
  d = doms;
  initCore(d);
  initAnnotate(d);
  initShot(d);
  setupDrag();
  setupPanel();
  setupToolbar();
  setupListeners();
}

function setupDrag() {
  let dragging = false, dragX = 0, dragY = 0, startX = 0, startY = 0;
  d.btn.addEventListener("pointerdown", (e: PointerEvent) => {
    dragging = true; d.btn.classList.add("dragging"); d.btn.setPointerCapture(e.pointerId);
    startX = e.clientX; startY = e.clientY;
    const r = d.btn.getBoundingClientRect(); dragX = r.left; dragY = r.top;
  });
  d.btn.addEventListener("pointermove", (e: PointerEvent) => {
    if (!dragging) return;
    d.btn.style.left = Math.max(0, Math.min(dragX + e.clientX - startX, window.innerWidth - 24)) + "px";
    d.btn.style.top = Math.max(0, Math.min(dragY + e.clientY - startY, window.innerHeight - 24)) + "px";
    d.btn.style.right = "auto"; d.btn.style.bottom = "auto";
  });
  d.btn.addEventListener("pointerup", (e: PointerEvent) => {
    dragging = false; d.btn.classList.remove("dragging"); d.btn.releasePointerCapture(e.pointerId);
    if (Math.abs(e.clientX - startX) + Math.abs(e.clientY - startY) < 5) togglePanel(true);
  });
}

function setupPanel() {
  d.sendBtn.addEventListener("click", sendAll);
  d.closeBtn.addEventListener("click", () => togglePanel(false));
  d.cancelBtn.addEventListener("click", () => togglePanel(false));
  d.overlay.addEventListener("click", (e) => {
    if ((e.target as HTMLElement).closest(".pw-mp-shadow")) return;
    if (state.pendingEl) finishAnnotation(true);
    state.setMode(""); updateToolbar(); togglePanel(false);
  });
}

function setupToolbar() {
  d.toolbarEl.addEventListener("click", (e) => {
    const b = (e.target as HTMLElement).closest("button[data-mode]") as HTMLButtonElement | null;
    if (!b) return;
    if (b.dataset.mode === "screenshot") { activateShot(); return; }
    state.setMode(state.mode === "annotate" ? "" : "annotate"); updateToolbar();
    if (state.mode === "annotate") togglePanel(false);
  });
}

function setupListeners() {
  document.addEventListener("mousemove", (e: MouseEvent) => {
    if (state.mode !== "annotate" || state.frozen) { d.hlEl.style.display = "none"; return; }
    if (state.pendingEl || isPanel(e)) { d.hlEl.style.display = "none"; return; }
    const el = document.elementFromPoint(e.clientX, e.clientY);
    if (!el || el === document.body || el === document.documentElement) { d.hlEl.style.display = "none"; return; }
    const r = el.getBoundingClientRect();
    if (r.width === 0 || r.height === 0) { d.hlEl.style.display = "none"; return; }
    d.hlEl.style.cssText = `left:${r.left}px;top:${r.top}px;width:${r.width}px;height:${r.height}px;display:block`;
    const tag = el.tagName.toLowerCase();
    const cls = typeof (el as HTMLElement).className === "string" ? (el as HTMLElement).className.split(" ").slice(0, 2).join(".") : "";
    const id = (el as HTMLElement).id ? "#" + (el as HTMLElement).id : "";
    d.hlLabel.textContent = `${tag}${id}${cls ? "." + cls : ""}`;
  });

  document.addEventListener("click", (e: MouseEvent) => {
    if (state.mode !== "annotate" || isPanel(e)) return;
    const target = e.target as HTMLElement;
    if (state.pendingEl) { e.preventDefault(); e.stopPropagation(); e.stopImmediatePropagation(); finishAnnotation(true); return; }
    e.preventDefault(); e.stopPropagation(); e.stopImmediatePropagation();
    if (!target || target === document.body || target === document.documentElement) return;
    pickElement(target);
  }, true);

  document.addEventListener("contextmenu", (e: MouseEvent) => {
    if (state.mode !== "annotate") return;
    e.preventDefault();
    state.setMode(""); updateToolbar(); d.hlEl.style.display = "none"; notify("批注模式已退出", "ok");
  });

  document.addEventListener("dblclick", (e: MouseEvent) => {
    if (state.mode !== "annotate" || isPanel(e)) return;
    e.preventDefault();
    state.setMode(""); updateToolbar(); d.hlEl.style.display = "none"; notify("批注模式已退出", "ok");
  });
}