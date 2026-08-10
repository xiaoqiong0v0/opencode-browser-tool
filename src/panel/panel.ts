/// <reference types="../../node_modules/@types/node/index.d.ts" preserve="true" />

// Magic Panel 入口 — esbuild 打包为单文件 IIFE
import { createDom } from "./dom";
import { init } from "./handlers";
import { togglePanel } from "./core";
import * as state from "./state";

function initPanel() {
  const doms = createDom();
  (window as any).__pwMagicPanel = {
    showNotification(msg: string, type: string = "ok") {
      const notifyEl = doms.notifyEl;
      notifyEl.textContent = msg;
      notifyEl.className = "pw-mp-notify " + type + " show";
      const id = window.setTimeout(() => notifyEl.classList.remove("show"), 3000);
      (notifyEl as any)._timer = id;
    },
    show() {
      togglePanel(true);
    },
    hide() {
      togglePanel(false);
    },
    toggle() {
      togglePanel(!state.isActive);
    },
  };
  init(doms);
}

if (document.body) {
  initPanel();
} else {
  document.addEventListener("DOMContentLoaded", initPanel);
}
