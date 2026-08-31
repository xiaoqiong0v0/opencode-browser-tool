// 面板 Webview 入口:AI 功能区(批注/截图/记录/发送) + 配置区(设备/开发者工具/主题)
// 与 Rust 通过 Tauri event/invoke 通信,设备/开发者工具走 HTTP /api/*
import { createIcons, Camera, PenLine, Send, X } from "lucide";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { AnnotationRecord } from "./types";
import { applyTheme, getTheme, initTheme, saveTheme } from "./theme";

/** DOM 引用集合 */
interface PanelDoms {
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
    // 双 tab 切换:AI 功能区 / 配置区(导航样式)
    const tabAi = document.getElementById("tab-ai") as HTMLButtonElement;
    const tabConfig = document.getElementById("tab-config") as HTMLButtonElement;
    this.switchTab("ai");
    tabAi.addEventListener("click", () => this.switchTab("ai"));
    tabConfig.addEventListener("click", () => this.switchTab("config"));
    // 每次打开面板默认回到 AI 功能页(tabs-changed 广播面板开关状态)
    void listen<{ panel_open: boolean }>("tabs-changed", (e) => {
      if (e.payload?.panel_open) this.switchTab("ai");
    });

    // 底部发送按钮:发送所有记录(批注+截图),结果用统一 toast 通知
    this.doms.btnSend.addEventListener("click", () => {
      void invoke<boolean>("panel_cmd", { cmd: "send-all" }).then((sent) => {
        if (sent) {
          this.notify("已发送所有记录", "ok");
        } else {
          this.notify("无记录可发送", "bad");
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
    // Rust 推送模式状态(批注/截图按钮已移至工具栏,面板无需同步)
    void listen<{ annotate: boolean; shot: boolean }>("annotate-state", (e) => {
      // 保留监听占位(工具栏处理激活态)
      void e;
    });
  }

  /** 切换顶部导航 tab(AI 功能区 / 配置区) */
  private switchTab(which: "ai" | "config"): void {
    const tabAi = document.getElementById("tab-ai") as HTMLButtonElement;
    const tabConfig = document.getElementById("tab-config") as HTMLButtonElement;
    const paneAi = document.getElementById("tab-ai-pane") as HTMLDivElement;
    const paneConfig = document.getElementById("tab-config-pane") as HTMLDivElement;
    tabAi.classList.toggle("active", which === "ai");
    tabConfig.classList.toggle("active", which === "config");
    paneAi.hidden = which !== "ai";
    paneConfig.hidden = which !== "config";
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
    void invoke("toolbar_close_panel");
  }

  /** 统一通知气泡(与批注/截图一致),type: ok|bad|err */
  private notify(message: string, type?: string): void {
    const el = document.getElementById("toast") as HTMLDivElement;
    el.textContent = message;
    el.className = "show " + (["ok", "bad", "err"].includes(type || "") ? type : "ok");
    clearTimeout((el as unknown as { _t?: number })._t);
    (el as unknown as { _t?: number })._t = window.setTimeout(() => {
      el.className = "";
    }, 3000);
  }

  /** 渲染记录列表(区分批注/截图,截图带缩略图;点击弹出详情) */
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
      if (r.url) {
        const urlEl = document.createElement("div");
        urlEl.className = "rec-url";
        urlEl.textContent = r.url;
        item.appendChild(urlEl);
      }
      if (r.note) {
        const note = document.createElement("div");
        note.className = "rec-note";
        note.textContent = r.note;
        item.appendChild(note);
      }
      if (r.type === "screenshot" && r.image) {
        const img = document.createElement("img");
        img.className = "rec-thumb";
        img.src = toDataUrl(r.image);
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
      // 默认不选中任何设备(placeholder 提示"设备预设...",用户选择后才应用)
    } catch (e) {
      console.error("[panel] load devices failed:", e);
    }
  }
}

const panel = new Panel();
void panel.init();
void initTheme();
// lucide 图标替换(批注/截图/发送/关闭)
createIcons({ icons: { PenLine, Camera, Send, X } });
console.log("[panel] ready");

/** 截图 base64 转可显示 data URL(裸 base64 需补前缀) */
function toDataUrl(base64: string): string {
  return base64.startsWith("data:") ? base64 : `data:image/png;base64,${base64}`;
}
