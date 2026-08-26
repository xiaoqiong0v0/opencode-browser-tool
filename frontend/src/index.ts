// 面板 Webview 入口:AI 功能区(批注/截图/记录/发送) + 配置区(设备/开发者工具/主题)
// 与 Rust 通过 Tauri event/invoke 通信,设备/开发者工具走 HTTP /api/*
import { createIcons, Camera, PenLine, Send, X } from "lucide";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { AnnotationRecord } from "./types";
import { applyTheme, getTheme, initTheme, saveTheme } from "./theme";

/** DOM 引用集合 */
interface PanelDoms {
  btnAnnotate: HTMLButtonElement;
  btnShot: HTMLButtonElement;
  btnSend: HTMLButtonElement;
  btnDevtools: HTMLButtonElement;
  btnPanelClose: HTMLButtonElement;
  deviceSelect: HTMLSelectElement;
  themeSelect: HTMLSelectElement;
  records: HTMLDivElement;
}

class Panel {
  private doms: PanelDoms;
  private records: AnnotationRecord[] = [];
  private servicePort = 0;

  constructor() {
    this.doms = {
      btnAnnotate: document.getElementById("btn-annotate") as HTMLButtonElement,
      btnShot: document.getElementById("btn-shot") as HTMLButtonElement,
      btnSend: document.getElementById("btn-send") as HTMLButtonElement,
      btnDevtools: document.getElementById("btn-devtools") as HTMLButtonElement,
      btnPanelClose: document.getElementById("btn-panel-close") as HTMLButtonElement,
      deviceSelect: document.getElementById("device-select") as HTMLSelectElement,
      themeSelect: document.getElementById("theme-select") as HTMLSelectElement,
      records: document.getElementById("records") as HTMLDivElement,
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

    // 顶部功能按钮:批注/截图(进入模式后自动收起面板,模式状态由 annotate-state 同步)
    this.doms.btnAnnotate.addEventListener("click", () => {
      void invoke<boolean>("panel_cmd", { cmd: "toggle-annotate" }).then(() => this.closePanel());
    });
    this.doms.btnShot.addEventListener("click", () => {
      void invoke<boolean>("panel_cmd", { cmd: "toggle-shot" }).then(() => this.closePanel());
    });
    // 底部发送按钮:发送所有记录(批注+截图)
    this.doms.btnSend.addEventListener("click", () => {
      void invoke<boolean>("panel_cmd", { cmd: "send-all" }).then((sent) => {
        const el = document.getElementById("send-result");
        if (el) {
          el.textContent = sent ? "已发送所有记录" : "无记录可发送";
          el.className = sent ? "send-ok" : "send-empty";
        }
      });
    });
    // 设备切换(选择后应用预设,并关闭面板显示效果)
    this.doms.deviceSelect.addEventListener("change", () => {
      const name = this.doms.deviceSelect.value;
      if (!name) return;
      void this.api("/api/device", { name })
        .then(() => this.closePanel())
        .catch((e) => {
          console.error("[panel] apply device failed:", e);
          this.closePanel();
        });
    });
    // 开发者工具开关(操作后关闭面板)
    this.doms.btnDevtools.addEventListener("click", () => {
      void this.api("/api/devtools", { action: "toggle" })
        .then(() => this.closePanel())
        .catch((e) => {
          console.error("[panel] devtools failed:", e);
          this.closePanel();
        });
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
    // Rust 推送模式状态(批注/截图按钮激活态)
    void listen<{ annotate: boolean; shot: boolean }>("annotate-state", (e) => {
      this.syncMode(e.payload);
    });
  }

  /** 同步批注/截图按钮激活态 */
  private syncMode(m: { annotate: boolean; shot: boolean }): void {
    this.doms.btnAnnotate.classList.toggle("active", !!m.annotate);
    this.doms.btnShot.classList.toggle("active", !!m.shot);
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

  /** 关闭面板:面板内操作(批注/截图/设备/开发者工具等)执行后统一收起 */
  private closePanel(): void {
    void invoke<boolean>("toolbar_toggle_panel");
  }

  /** 渲染记录列表(区分批注/截图,截图带缩略图) */
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
      const head = document.createElement("div");
      head.className = "rec-head";
      const tag = document.createElement("span");
      tag.className = "rec-tag " + r.type;
      tag.textContent = r.type === "screenshot" ? "截图" : "批注";
      const title = document.createElement("span");
      title.className = "rec-title";
      title.textContent = r.type === "screenshot" ? `#${r.index} 区域截图` : `#${r.index} ${r.selector}`;
      head.appendChild(tag);
      head.appendChild(title);
      item.appendChild(head);
      if (r.note) {
        const note = document.createElement("div");
        note.className = "rec-note";
        note.textContent = r.note;
        item.appendChild(note);
      }
      if (r.type === "screenshot" && r.image) {
        const img = document.createElement("img");
        img.className = "rec-thumb";
        img.src = r.image;
        img.alt = "screenshot";
        item.appendChild(img);
      }
      el.appendChild(item);
    }
  }

  /** 主动拉取记录/模式状态/设备列表(启动时) */
  async init(): Promise<void> {
    try {
      this.records = await invoke<AnnotationRecord[]>("panel_records");
      this.render();
    } catch (e) {
      console.error("[panel] init failed:", e);
    }
    // 恢复批注/截图按钮激活态
    try {
      const m = await invoke<{ annotate: boolean; shot: boolean }>("panel_mode");
      this.syncMode(m);
    } catch (e) {
      console.error("[panel] load mode failed:", e);
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
// lucide 图标替换(批注/截图/发送/关闭),key 需用 PascalCase(lucide 内部转 PascalCase 查表)
createIcons({ icons: { PenLine, Camera, Send, X } });
console.log("[panel] ready");
