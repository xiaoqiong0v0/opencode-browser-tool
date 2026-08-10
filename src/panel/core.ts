import { s } from "./config";
import { post } from "./utils";
import * as state from "./state";
import type { Dom } from "./handlers";
import { BRIDGE, SESSION_ID } from "./config";

let d: Dom;

export function initCore(doms: Dom) {
  d = doms;
}

export function notify(msg: string, type: string = "ok") {
  d.notifyEl.textContent = msg;
  d.notifyEl.className = "pw-mp-notify " + type + " show";
  const id = window.setTimeout(() => d.notifyEl.classList.remove("show"), 3000);
  (d.notifyEl as any)._timer = id;
}

export function closeEditBox() {
  d.editBox.classList.remove("active");
  d.editTextarea.value = "";
  state.setPendingEl(null);
  state.setFrozen(false);
}

let cursorStyle: HTMLStyleElement | null = null;

export function updateToolbar() {
  d.toolbarEl.querySelectorAll("button").forEach((b) => {
    b.classList.toggle("active", b.dataset.mode === state.mode);
  });
  d.hlEl.style.display = state.mode === "annotate" ? "block" : "none";
  if (state.mode === "annotate" && !cursorStyle) {
    cursorStyle = document.createElement("style");
    cursorStyle.id = "pw-cursor";
    cursorStyle.textContent = "body *:not(.pw-mp-overlay *,.pw-mp-btn,.pw-mp-editbox *){cursor:default!important}";
    document.head.appendChild(cursorStyle);
  } else if (state.mode !== "annotate" && cursorStyle) {
    cursorStyle.remove();
    cursorStyle = null;
  }
  if (state.mode === "") closeEditBox();
}

export function finishAnnotation(forceSave?: boolean) {
  if (!state.pendingEl) return;
  const val = d.editTextarea.value.trim();
  const { id } = state.pendingEl;
  if (forceSave === false) {
    post("/annotate/discard", { id });
  } else if (val) {
    post("/annotate/update", { id, annotation: val });
    notify(s("panel.annotation_stored"), "ok");
  } else {
    post("/annotate/discard", { id });
  }
  state.setPendingEl(null);
  closeEditBox();
  state.setFrozen(false);
  refreshRecords();
}

export function togglePanel(show: boolean, instant?: boolean) {
  if (instant) {
    d.overlay.style.transition = "none";
    const pw = d.overlay.querySelector<HTMLElement>(".pw-mp-shadow");
    if (pw) pw.style.transition = "none";
  }
  state.setIsActive(show);
  d.overlay.classList.toggle("active", show);
  d.btn.classList.toggle("hidden", show);
  if (!show) {
    d.hlEl.style.display = "none";
    closeEditBox();
  } else {
    d.urlEl.textContent = window.location.href;
    if (state.mode === "annotate") {
      state.setMode("");
      updateToolbar();
    }
    refreshRecords();
    updateToolbar();
  }
  if (instant) {
    d.overlay.offsetHeight;
    d.overlay.style.transition = "";
    const pw = d.overlay.querySelector<HTMLElement>(".pw-mp-shadow");
    if (pw) pw.style.transition = "";
  }
}

function el(tag: string, cls: string = "", ...children: (string | Node)[]): HTMLElement {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  for (const c of children) {
    if (typeof c === "string") e.appendChild(document.createTextNode(c));
    else e.appendChild(c);
  }
  return e;
}

function recordItem(r: any): HTMLElement {
  const isShot = r.type === "screenshot";
  const item = el("div", "pw-mp-rec");
  const header = el("div", "pw-mp-rec-header");
  const icon = el("span", "", isShot ? "📷" : "✏️");
  icon.style.color = isShot ? "#e94560" : "#00b4ff";
  header.append(icon, el("span", "pw-mp-rec-num", `#${r.id}`));
  const body = el("span", "", r.type === "screenshot" ? r.annotation || "" : r.tag || r.content || "");
  body.style.cssText = "color:#ccc;font-size:12px;flex:1";
  header.appendChild(body);
  item.appendChild(header);
  if (r.base64) {
    const img = el("img", "pw-mp-rec-img") as HTMLImageElement;
    img.src = `data:image/png;base64,${r.base64}`;
    img.alt = "screenshot";
    item.appendChild(img);
  }
  if (r.annotation) item.appendChild(el("div", "pw-mp-rec-anno", r.annotation));
  const del = el("button", "pw-mp-rec-del", "✕");
  del.addEventListener("click", async (e) => {
    e.stopPropagation();
    item.remove();
    await post("/annotate/discard", { id: r.id });
    refreshRecords();
  });
  item.appendChild(del);
  return item;
}

export async function refreshRecords() {
  const resp = await fetch(`${BRIDGE}/records?sessionId=${SESSION_ID}`);
  const data = await resp.json();
  d.listEl.innerHTML = "";
  if (!data.success || !data.records || data.records.length === 0) {
    d.listEl.innerHTML = `<div class="pw-mp-empty">${s("panel.no_records")}</div>`;
    d.sendBtn.disabled = true;
    return;
  }
  for (const r of data.records) d.listEl.appendChild(recordItem(r));
  d.sendBtn.disabled = false;
}

export async function sendAll() {
  // 先保存当前未完成的批注
  if (state.pendingEl) {
    const val = d.editTextarea.value.trim();
    if (!val) {
      notify(s("panel.annotation_required"), "bad");
      return;
    }
    post("/annotate/update", { id: state.pendingEl.id, annotation: val });
    state.setPendingEl(null);
    closeEditBox();
  }
  try {
    const resp = await post("/send-all");
    if (resp.count > 0) {
      notify(s("panel.sent_records").replace("{n}", String(resp.count)), "ok");
      refreshRecords();
    } else {
      notify(s("panel.send_records_failed"), "bad");
    }
  } catch {
    notify(s("panel.send_records_failed"), "err");
  }
}
