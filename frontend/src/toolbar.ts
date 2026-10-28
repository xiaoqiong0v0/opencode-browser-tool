// 工具栏 Webview 入口:标签(伪多标签) + 地址栏导航 + 批注/截图/下载/面板按钮
// 与 Rust 通过 Tauri invoke 通信
import {
  createIcons,
  ArrowLeft,
  ArrowRight,
  Camera,
  ChevronLeft,
  ChevronRight,
  Copy,
  Download,
  Globe,
  Minus,
  PenLine,
  Plus,
  RotateCw,
  Settings,
  Square,
  X,
} from "lucide";
import { invoke } from "@tauri-apps/api/core";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { listen } from "@tauri-apps/api/event";
import { initTheme } from "./theme";
import { postApi } from "./api-client";
import { countInProgressDownloads, downloadEventTitle, downloadsBadgeText, isDownloadStart } from "./download-badge";
import { isNewTabUrl, resolveAddrBlurValue } from "./addr-bar";

/** 角标刷新节流间隔(ms):同一批下载事件合并成一次 `/api/downloads` 请求 */
const BADGE_REFRESH_MS = 500;

/** 角标脉冲动画时长(ms):与 toolbar.html 的 `@keyframes dl-badge-pulse` 同步维护 */
const BADGE_PULSE_MS = 1000;

interface TabItem {
  id: number;
  url: string;
  title: string;
  icon: string;
}

interface ToolbarState {
  tabs: TabItem[];
  active: number;
  panel_open: boolean;
}

/** lucide 图标集合:静态按钮 + 动态渲染(标签图标/标签关闭/窗口最大化还原)共用 */
const ICONS = {
  ArrowLeft,
  ArrowRight,
  Camera,
  ChevronLeft,
  ChevronRight,
  Copy,
  Download,
  Globe,
  Minus,
  PenLine,
  Plus,
  RotateCw,
  Settings,
  Square,
  X,
};

class Toolbar {
  private tabsEl: HTMLDivElement;
  private addr: HTMLInputElement;
  private addrClear: HTMLButtonElement;
  private btnPanel: HTMLButtonElement;
  private btnAnnotate: HTMLButtonElement;
  private btnShot: HTMLButtonElement;
  private btnDownloads: HTMLButtonElement;
  private dlBadge: HTMLSpanElement;
  private state: ToolbarState = { tabs: [], active: 0, panel_open: false };
  /** 上次角标刷新时间戳与节流定时器(见 scheduleBadgeRefresh) */
  private badgeRefreshAt = 0;
  private badgeRefreshTimer = 0;
  /** 角标请求序号:只接受最新一次请求的响应,避免慢响应覆盖新结果 */
  private badgeReqId = 0;
  /** 角标脉冲动画的移除定时器 */
  private badgePulseTimer = 0;

  constructor() {
    this.tabsEl = document.getElementById("tabs") as HTMLDivElement;
    this.addr = document.getElementById("addr") as HTMLInputElement;
    this.addrClear = document.getElementById("addr-clear") as HTMLButtonElement;
    this.btnPanel = document.getElementById("btn-panel") as HTMLButtonElement;
    this.btnAnnotate = document.getElementById("btn-annotate") as HTMLButtonElement;
    this.btnShot = document.getElementById("btn-shot") as HTMLButtonElement;
    this.btnDownloads = document.getElementById("btn-downloads") as HTMLButtonElement;
    this.dlBadge = document.getElementById("dl-badge") as HTMLSpanElement;
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
    // 下载角标:独立初始化,事件通道不可用(旧 shell / 非 Tauri)只告警,不影响其它功能
    void this.initDownloads();
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
    this.syncAddrClear();
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

  /**
   * 下载角标初始化:监听 `downloads-changed` + 首帧统计一次进行中数量
   *
   * 监听失败(非 Tauri / 旧 shell 无事件通道)只告警,不抛错、不影响其它功能;
   * 工具栏常驻,面板是否可见与本刷新无关。
   */
  private async initDownloads(): Promise<void> {
    try {
      await listen("downloads-changed", (e) => this.onDownloadsChanged(e.payload));
    } catch (err) {
      console.warn("[toolbar] downloads-changed unavailable:", err);
    }
    void this.refreshBadge();
  }

  /**
   * 处理 `downloads-changed`:新下载开始 → 角标脉冲 + 更新 title;任何事件都触发一次计数刷新(节流)
   * @param payload 事件 data
   */
  private onDownloadsChanged(payload: unknown): void {
    if (isDownloadStart(payload)) {
      const title = downloadEventTitle(payload);
      if (title) this.btnDownloads.title = title;
      this.pulseBadge();
    }
    this.scheduleBadgeRefresh();
  }

  /** 角标脉冲 1s(先摘类 + 强制重排,确保连续新下载都能重新触发动画) */
  private pulseBadge(): void {
    this.dlBadge.classList.remove("pulse");
    void this.dlBadge.offsetWidth;
    this.dlBadge.classList.add("pulse");
    clearTimeout(this.badgePulseTimer);
    this.badgePulseTimer = window.setTimeout(() => this.dlBadge.classList.remove("pulse"), BADGE_PULSE_MS);
  }

  /**
   * 角标计数刷新(节流 ≥500ms)
   *
   * 窗口内的多余事件不额外发请求,而是在窗口末尾补一次,保证最终数量一定能刷到。
   */
  private scheduleBadgeRefresh(): void {
    const wait = BADGE_REFRESH_MS - (Date.now() - this.badgeRefreshAt);
    if (wait <= 0) {
      this.badgeRefreshAt = Date.now();
      void this.refreshBadge();
      return;
    }
    if (this.badgeRefreshTimer) return;
    this.badgeRefreshTimer = window.setTimeout(() => {
      this.badgeRefreshTimer = 0;
      this.badgeRefreshAt = Date.now();
      void this.refreshBadge();
    }, wait);
  }

  /** 拉取 `/api/downloads` 统计进行中数量并更新角标(只采纳最新一次请求的响应) */
  private async refreshBadge(): Promise<void> {
    const reqId = ++this.badgeReqId;
    try {
      const resp = await postApi("/api/downloads", {}, "toolbar");
      if (reqId !== this.badgeReqId) return;
      if (resp?.success === false) return;
      this.setBadgeCount(countInProgressDownloads(resp?.data));
    } catch (e) {
      console.warn("[toolbar] poll downloads failed:", e);
    }
  }

  /**
   * 渲染角标:显示进行中数量,0 条时隐藏
   * @param count 进行中下载数量
   */
  private setBadgeCount(count: number): void {
    const text = downloadsBadgeText(count);
    this.dlBadge.textContent = text;
    this.dlBadge.hidden = text === "";
  }

  /**
   * 同步清空按钮可见性:有输入时显示,空时隐藏(用 `hidden`,CSS 里配 `[hidden]{display:none}` 覆盖 flex)
   */
  private syncAddrClear(): void {
    this.addrClear.hidden = this.addr.value === "";
  }

  /**
   * 地址栏失焦:输入为空时恢复当前激活标签的 URL(拿不到 URL / 输入非空 → 不动)
   *
   * 写回 `value` 不会自动全选,光标落在末尾,下次输入即替换这段恢复的文本。
   */
  private restoreAddrOnBlur(): void {
    const activeTab = this.state.tabs.find((t) => t.id === this.state.active);
    const restored = resolveAddrBlurValue(this.addr.value, activeTab?.url);
    if (restored === null || restored === this.addr.value) return;
    this.addr.value = restored;
    this.addr.setSelectionRange(restored.length, restored.length);
    this.syncAddrClear();
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
    // 地址栏输入:切换清空按钮显隐
    this.addr.addEventListener("input", () => this.syncAddrClear());
    // 地址栏失焦:空输入时恢复当前页面 URL
    this.addr.addEventListener("blur", () => this.restoreAddrOnBlur());
    // 清空按钮:mousedown 阻止默认,避免输入框先失焦触发「空则恢复」;click 清空并聚焦
    this.addrClear.addEventListener("mousedown", (e) => e.preventDefault());
    this.addrClear.addEventListener("click", () => {
      this.addr.value = "";
      this.syncAddrClear();
      this.addr.focus();
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
    // 下载按钮:Rust 负责显示面板 + 广播 downloads-view-requested(旧 shell 无此命令时只告警)
    this.btnDownloads.addEventListener("click", () => {
      void invoke("panel_show_downloads").catch((e) => {
        console.warn("[toolbar] panel_show_downloads failed:", e);
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
        // 还原=copy 图标,最大化=square 图标;动态改写后需重新替换为内联 SVG
        maxBtn.innerHTML = (await win.isMaximized()) ? '<i data-lucide="copy"></i>' : '<i data-lucide="square"></i>';
        createIcons({ icons: ICONS });
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

      // 页面图标:用页面自身图标地址(tab.icon),为空时显示默认 globe 图标
      const fav = document.createElement("span");
      fav.className = "tab-fav";
      if (t.icon) {
        const img = document.createElement("img");
        img.src = t.icon;
        img.style.cssText = "width:14px;height:14px";
        img.onerror = () => {
          fav.innerHTML = '<i data-lucide="globe"></i>';
          createIcons({ icons: ICONS });
        };
        fav.appendChild(img);
      } else {
        fav.innerHTML = '<i data-lucide="globe"></i>';
      }
      top.appendChild(fav);

      const label = document.createElement("span");
      label.className = "tab-title";
      label.textContent = t.title || t.url || "新标签页";
      top.appendChild(label);

      const close = document.createElement("span");
      close.className = "tab-close";
      close.innerHTML = '<i data-lucide="x"></i>';
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
    // 动态生成的标签图标/关闭按钮替换为内联 SVG(静态按钮在模块底部已替换)
    createIcons({ icons: ICONS });
  }
}

new Toolbar();
void initTheme();
// lucide 图标替换(静态按钮:批注/截图/下载/新建/导航/窗口/面板/折叠)
createIcons({ icons: ICONS });
console.log("[toolbar] ready");
