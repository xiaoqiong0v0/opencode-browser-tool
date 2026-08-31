// 工具栏 Webview 入口:标签(伪多标签) + 地址栏导航 + 批注/截图/面板按钮
// 与 Rust 通过 Tauri invoke 通信
import { createIcons, Camera, PenLine } from "lucide";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { listen } from "@tauri-apps/api/event";
import { initTheme } from "./theme";

interface TabItem {
  id: number;
  url: string;
  title: string;
}

interface ToolbarState {
  tabs: TabItem[];
  active: number;
  panel_open: boolean;
}

class Toolbar {
  private tabsEl: HTMLDivElement;
  private addr: HTMLInputElement;
  private btnPanel: HTMLButtonElement;
  private btnAnnotate: HTMLButtonElement;
  private btnShot: HTMLButtonElement;
  private state: ToolbarState = { tabs: [], active: 0, panel_open: false };

  constructor() {
    this.tabsEl = document.getElementById("tabs") as HTMLDivElement;
    this.addr = document.getElementById("addr") as HTMLInputElement;
    this.btnPanel = document.getElementById("btn-panel") as HTMLButtonElement;
    this.btnAnnotate = document.getElementById("btn-annotate") as HTMLButtonElement;
    this.btnShot = document.getElementById("btn-shot") as HTMLButtonElement;
    this.bindEvents();
    void this.init();
  }

  /** 事件驱动:监听 Rust 广播的标签状态变化(后台统一检测,有变更才发送) */
  private async init(): Promise<void> {
    await listen<ToolbarState>("tabs-changed", (e) => this.update(e.payload));
    // 监听批注/截图模式状态(按钮激活态同步)
    void listen<{ annotate: boolean; shot: boolean }>("annotate-state", (e) => {
      this.btnAnnotate.classList.toggle("active", !!e.payload?.annotate);
      this.btnShot.classList.toggle("active", !!e.payload?.shot);
    });
    // 初始拉取一次当前状态
    void this.refresh();
  }

  private update(st: ToolbarState): void {
    this.state = st;
    this.render();
    // 激活标签的 URL 回填地址栏(仅当聚焦非激活状态,避免打断输入)
    const activeTab = this.state.tabs.find((t) => t.id === this.state.active);
    if (activeTab && document.activeElement !== this.addr) {
      // 新标签页特殊处理:地址栏显示为空
      this.addr.value = isNewTabUrl(activeTab.url) ? "" : activeTab.url;
    }
    this.btnPanel.classList.toggle("active", this.state.panel_open);
  }

  private async refresh(): Promise<void> {
    try {
      const st = await invoke<ToolbarState>("toolbar_state");
      this.update(st);
    } catch {
      // 服务未就绪时忽略
    }
  }

  private bindEvents(): void {
    // 地址栏回车导航
    this.addr.addEventListener("keydown", (e) => {
      if (e.key !== "Enter") return;
      const url = this.addr.value.trim();
      if (!url) return;
      void invoke("toolbar_navigate", { url }).then(() => {
        // 导航后失焦 + 刷新:显示补全协议后的完整 URL
        this.addr.blur();
        void this.refresh();
      });
    });
    // 面板浮层开关
    this.btnPanel.addEventListener("click", () => {
      void invoke<boolean>("toolbar_toggle_panel").then((open) => {
        this.btnPanel.classList.toggle("active", open);
      });
    });
    // 批注模式快捷按钮(免开面板)
    this.btnAnnotate.addEventListener("click", () => {
      void invoke<boolean>("panel_cmd", { cmd: "toggle-annotate" }).then((on) => {
        this.btnAnnotate.classList.toggle("active", on);
      });
    });
    // 截图快捷按钮(立即截全屏,不进模式)
    this.btnShot.addEventListener("click", () => {
      void invoke<boolean>("panel_cmd", { cmd: "shot-now" }).then(() => {
        // 如果面板开着,关闭它(截图预览在 overlay 显示)
        if (this.state.panel_open) {
          void invoke<boolean>("toolbar_toggle_panel");
        }
      });
    });
    // 新建标签
    document.getElementById("btn-add")!.addEventListener("click", () => {
      void invoke("toolbar_new_tab", {}).then(() => void this.refresh());
    });
    // 后退/前进/刷新
    document.getElementById("btn-back")!.addEventListener("click", () => {
      void invoke("toolbar_go_back").then(() => void this.refresh());
    });
    document.getElementById("btn-forward")!.addEventListener("click", () => {
      void invoke("toolbar_go_forward").then(() => void this.refresh());
    });
    document.getElementById("btn-reload")!.addEventListener("click", () => {
      void invoke("toolbar_reload").then(() => void this.refresh());
    });
    // 标签折叠:左右滚动
    document.getElementById("tab-scroll-left")!.addEventListener("click", () => {
      this.tabsEl.scrollBy({ left: -160, behavior: "smooth" });
    });
    document.getElementById("tab-scroll-right")!.addEventListener("click", () => {
      this.tabsEl.scrollBy({ left: 160, behavior: "smooth" });
    });
    // 窗口控制按钮(无系统标题栏)
    const win = getCurrentWindow();
    const maxBtn = document.getElementById("win-max") as HTMLDivElement;
    const updateMaxIcon = async () => {
      try {
        maxBtn.textContent = (await win.isMaximized()) ? "❐" : "□";
      } catch {
        // 权限或查询失败时保持默认图标
      }
    };
    document.getElementById("win-min")!.addEventListener("click", () => void win.minimize());
    maxBtn.addEventListener("click", () => void win.toggleMaximize().then(() => void updateMaxIcon()));
    document.getElementById("win-close")!.addEventListener("click", () => void win.close());
    void win.onResized(updateMaxIcon);
    void updateMaxIcon();
  }

  private render(): void {
    this.tabsEl.innerHTML = "";
    for (const t of this.state.tabs) {
      const chip = document.createElement("div");
      chip.className = "tab" + (t.id === this.state.active ? " active" : "");
      chip.title = t.url;

      // 标签主体:图标 + 标题 + × 包起来
      const top = document.createElement("div");
      top.className = "tab-top";

      // 页面图标:取 URL 域名 favicon,失败显示默认图标
      const fav = document.createElement("span");
      fav.className = "tab-fav";
      fav.textContent = "🌐";
      const host = this.hostOf(t.url);
      if (host) {
        const img = document.createElement("img");
        img.src = `https://www.google.com/s2/favicons?domain=${encodeURIComponent(host)}&sz=32`;
        img.style.cssText = "width:14px;height:14px";
        img.onerror = () => { fav.textContent = "🌐"; };
        fav.textContent = "";
        fav.appendChild(img);
      }
      top.appendChild(fav);

      const label = document.createElement("span");
      label.className = "tab-title";
      label.textContent = t.title || t.url || "新标签页";
      top.appendChild(label);

      const close = document.createElement("span");
      close.className = "tab-close";
      close.textContent = "×";
      close.addEventListener("click", (e) => {
        e.stopPropagation();
        void invoke("toolbar_close_tab", { id: t.id }).then(() => void this.refresh());
      });
      top.appendChild(close);

      chip.appendChild(top);

      // 底部连接条(激活时显示,连接地址行)
      const link = document.createElement("div");
      link.className = "tab-link";
      chip.appendChild(link);

      chip.addEventListener("click", () => {
        if (t.id !== this.state.active) {
          void invoke("toolbar_switch_tab", { id: t.id }).then(() => void this.refresh());
        }
      });
      this.tabsEl.appendChild(chip);
    }
  }

  /** 从 URL 提取域名(用于 favicon),无效返回空 */
  private hostOf(url: string): string {
    try {
      return new URL(url).hostname;
    } catch {
      return "";
    }
  }
}

/** 判断是否新标签页(about:blank / tauri 内部 newtab 地址),地址栏应显示为空 */
function isNewTabUrl(url: string): boolean {
  return url === "about:blank" || url.includes("/newtab.html") || url.includes("tauri://localhost") || url.includes("tauri.localhost");
}

new Toolbar();
void initTheme();
// lucide 图标替换(批注/截图)
createIcons({ icons: { PenLine, Camera } });
console.log("[toolbar] ready");
