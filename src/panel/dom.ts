import { s } from "./config";
import { escHtml } from "./utils";
import STYLES from "./styles.css";

function h<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  cls: string,
  parent?: HTMLElement | ShadowRoot,
): HTMLElementTagNameMap[K] {
  const el = document.createElement(tag);
  el.className = cls;
  parent?.appendChild(el);
  return el;
}

export function createDom() {
  const root = document.createElement("div");
  root.id = "pw-mp-root";
  root.style.cssText = "all:initial;position:fixed !important;inset:0 !important;z-index:2147483647 !important;pointer-events:none;width:100vw;height:100vh";
  document.body.appendChild(root);

  const shadow = root.attachShadow({ mode: "closed" });
  const style = document.createElement("style");
  style.textContent = STYLES;
  shadow.appendChild(style);

  const btn = h("button", "pw-mp-btn", shadow);
  btn.title = s("panel.title");
  btn.style.pointerEvents = "auto";

  const overlay = h("div", "pw-mp-overlay", shadow);
  const shadowHost = h("div", "pw-mp-shadow", overlay);
  const shadowRoot = shadowHost.attachShadow({ mode: "closed" });
  const shadowStyle = document.createElement("style");
  shadowStyle.textContent = STYLES;
  shadowRoot.appendChild(shadowStyle);
  const sidebar = h("div", "pw-mp-sidebar", shadowRoot);
  const headerEl = h("div", "pw-mp-sidebar-header", sidebar);
  headerEl.innerHTML = `<span>✦ ${s("panel.title")}</span>`;
  const closeBtn = h("button", "pw-mp-sidebar-close", headerEl);
  closeBtn.textContent = "✕";
  closeBtn.style.pointerEvents = "auto";
  const toolbarEl = h("div", "pw-mp-toolbar", sidebar);
  toolbarEl.innerHTML = `<button data-mode="annotate">✏️ ${s("panel.annotate")}</button><button data-mode="screenshot">📷 ${s("panel.screenshot_btn")}</button>`;
  const urlEl = h("div", "pw-mp-url", sidebar);
  const listEl = h("div", "pw-mp-list", sidebar);
  const footerEl = h("div", "pw-mp-footer", sidebar);
  const cancelBtn = h("button", "pw-mp-btn-cancel", footerEl);
  cancelBtn.textContent = s("panel.close");
  cancelBtn.style.pointerEvents = "auto";
  const sendBtn = h("button", "pw-mp-btn-send", footerEl);
  sendBtn.textContent = s("panel.send_all");
  sendBtn.disabled = true;
  sendBtn.style.pointerEvents = "auto";

  const hlEl = h("div", "pw-mp-highlight", shadow);
  const hlLabel = h("div", "pw-mp-hl-label", hlEl);

  const editBox = h("div", "pw-mp-editbox", shadow);
  editBox.innerHTML = `<div class="pw-mp-edit-dlg"><textarea placeholder="${escHtml(s("panel.annotation_placeholder"))}"></textarea><div class="pw-mp-edit-actions"><button class="pw-mp-edit-confirm">✓</button><button class="pw-mp-edit-cancel">✕</button><button class="pw-mp-edit-del">🗑</button><button class="pw-mp-edit-send">✈</button></div></div>`;
  const editTextarea = editBox.querySelector("textarea") as HTMLTextAreaElement;
  const editSendBtn = editBox.querySelector(".pw-mp-edit-send") as HTMLButtonElement;
  const editCancelBtn = editBox.querySelector(".pw-mp-edit-cancel") as HTMLButtonElement;
  const editConfirmBtn = editBox.querySelector(".pw-mp-edit-confirm") as HTMLButtonElement;
  const editDelBtn = editBox.querySelector(".pw-mp-edit-del") as HTMLButtonElement;

  const notifyEl = h("div", "pw-mp-notify", shadow) as HTMLDivElement;
  const shotOverlay = h("div", "pw-mp-shot", shadow);

  return {
    root, btn, overlay, shadowRoot, sidebar, toolbarEl, closeBtn,
    urlEl, listEl, cancelBtn, sendBtn, hlEl, hlLabel,
    editBox, editTextarea, editSendBtn, editCancelBtn, editConfirmBtn, editDelBtn,
    notifyEl, shotOverlay,
  };
}