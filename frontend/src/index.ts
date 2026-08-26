// 面板 Webview 入口:批注列表 + 操作按钮 + 说明输入 + 设备切换/开发者工具/主题切换
// 与 Rust 通过 Tauri event/invoke 通信,设备/开发者工具走 HTTP /api/*
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { AnnotationRecord } from "./types";
import { applyTheme, getTheme, initTheme, saveTheme } from "./theme";

/** DOM 引用集合 */
interface PanelDoms {
  btnAnnotate: HTMLButtonElement;
  btnSend: HTMLButtonElement;
  btnDevtools: HTMLButtonElement;
  btnPanelClose: HTMLButtonElement;
  deviceSelect: HTMLSelectElement;
  themeSelect: HTMLSelectElement;
  records: HTMLDivElement;
  note: HTMLTextAreaElement;
}

class Panel {
  private doms: PanelDoms;
  private records: AnnotationRecord[] = [];
  private servicePort = 0;

  constructor() {
    this.doms = {
      btnAnnotate: document.getElementById("btn-annotate") as HTMLButtonElement,
      btnSend: document.getElementById("btn-send") as HTMLButtonElement,
      btnDevtools: document.getElementById("btn-devtools") as HTMLButtonElement,
      btnPanelClose: document.getElementById("btn-panel-close") as HTMLButtonElement,
      deviceSelect: document.getElementById("device-select") as HTMLSelectElement,
      themeSelect: document.getElementById("theme-select") as HTMLSelectElement,
      records: document.getElementById("records") as HTMLDivElement,
      note: document.getElementById("note") as HTMLTextAreaElement,
    };
    this.bindEvents();
  }

  private bindEvents(): void {
    // 双 tab 切换:AI 功能区 / 配置区
    const tabAi = document.getElementById("tab-ai") as HTMLButtonElement;
    const tabConfig = document.getElementById("tab-config") as HTMLButtonElement;
    const paneAi = document.getElementById("tab-ai-pane") as HTMLDivElement;
    const paneConfig = document.getElementById("tab-config-pane") as HTMLDivElement;
    const switchTab = (which: "ai" | "config") => {
      tabAi.classList.toggle("active", which === "ai");
      tabConfig.classList.toggle("active", which === "config");
      paneAi.hidden = which !== "ai";
      paneConfig.hidden = which !== "config";
    };
    tabAi.addEventListener("click", () => switchTab("ai"));
    tabConfig.addEventListener("click", () => switchTab("config"));

    this.doms.btnAnnotate.addEventListener("click", () => {
      void invoke<boolean>("panel_cmd", { cmd: "toggle-annotate" }).then((on) => {
        this.doms.btnAnnotate.classList.toggle("active", on);
      });
    });
    this.doms.btnSend.addEventListener("click", () => {
      void invoke<boolean>("panel_cmd", { cmd: "send-all" }).then((sent) => {
        const el = document.getElementById("send-result");
        if (el) {
          el.textContent = sent ? "已发送批注" : "无记录可发送";
          el.className = sent ? "send-ok" : "send-empty";
        }
      });
    });
    // 设备切换(选择后应用预设)
    this.doms.deviceSelect.addEventListener("change", () => {
      const name = this.doms.deviceSelect.value;
      if (!name) return;
      void this.api("/api/device", { name }).catch((e) => {
        console.error("[panel] apply device failed:", e);
      });
    });
    // 开发者工具开关
    this.doms.btnDevtools.addEventListener("click", () => {
      void this.api("/api/devtools", { action: "toggle" })
        .then((r) => {
          this.doms.btnDevtools.classList.toggle("active", !!r.data?.open);
        })
        .catch((e) => console.error("[panel] devtools failed:", e));
    });
    // 收起面板(覆盖式浮层关闭)
    this.doms.btnPanelClose.addEventListener("click", () => {
      void invoke<boolean>("toolbar_toggle_panel");
    });
    // 主题切换:本地持久化 + 广播所有 webview
    this.doms.themeSelect.value = getTheme();
    this.doms.themeSelect.addEventListener("change", () => {
      const mode = this.doms.themeSelect.value;
      saveTheme(mode);
      applyTheme(mode);
      void invoke("panel_set_theme", { theme: mode }).catch((e) => console.error("[panel] set theme failed:", e));
    });
    // Rust 推送记录变更
    void listen<AnnotationRecord[]>("records-changed", (e) => {
      this.records = e.payload || [];
      this.render();
    });
  }

  /** 调用本地 HTTP 服务 API */
  private async api(path: string, body?: unknown): Promise<any> {
    const port = this.servicePort || (await invoke<number>("panel_service_port"));
    this.servicePort = port;
    const res = await fetch(`http://127.0.0.1:${port}${path}`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body || {}),
    });
    return res.json();
  }

  /** 渲染记录列表 */
  private render(): void {
    const el = this.doms.records;
    el.innerHTML = "";
    if (this.records.length === 0) {
      const empty = document.createElement("div");
      empty.className = "empty";
      empty.textContent = "（无记录）";
      el.appendChild(empty);
      return;
    }
    for (const r of this.records) {
      const item = document.createElement("div");
      item.className = "rec";
      item.textContent = `#${r.index} ${r.selector}`;
      if (r.note) {
        const note = document.createElement("div");
        note.className = "rec-note";
        note.textContent = r.note;
        item.appendChild(note);
      }
      el.appendChild(item);
    }
  }

  /** 主动拉取记录(启动时) */
  async init(): Promise<void> {
    try {
      this.records = await invoke<AnnotationRecord[]>("panel_records");
      this.render();
    } catch (e) {
      console.error("[panel] init failed:", e);
    }
    // 加载设备列表
    try {
      const r = await this.api("/api/device/list");
      const devices = r.data?.devices || [];
      this.doms.deviceSelect.innerHTML = "";
      const placeholder = document.createElement("option");
      placeholder.value = "";
      placeholder.textContent = "设备预设...";
      this.doms.deviceSelect.appendChild(placeholder);
      for (const d of devices) {
        const opt = document.createElement("option");
        opt.value = d.name;
        opt.textContent = `${d.name} (${d.width}x${d.height})`;
        this.doms.deviceSelect.appendChild(opt);
      }
    } catch (e) {
      console.error("[panel] load devices failed:", e);
    }
  }
}

const panel = new Panel();
void panel.init();
void initTheme();
console.log("[panel] ready");
