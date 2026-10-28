// 面板 Webview 入口:AI 功能区(批注/截图/记录/发送) + 配置区(设备/开发者工具/主题)
// 与 Rust 通过 Tauri event/invoke 通信,设备/开发者工具走 HTTP /api/*
import {
  Bot,
  Camera,
  Download,
  FolderOpen,
  List,
  PenLine,
  RotateCcw,
  Save,
  Send,
  Settings,
  Trash2,
  Wrench,
  X,
  createIcons,
} from "lucide";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import type { AnnotationRecord } from "./types";
import { applyTheme, getTheme, initTheme, saveTheme } from "./theme";
import { decideDownloadsChanged } from "./download-events";
import type { DownloadsChangedPayload } from "./download-events";
import { postApi } from "./api-client";

/** 下载条目(与 Rust /api/downloads 的 camelCase 字段一致) */
interface DownloadItemView {
  id: string;
  url: string;
  filename: string;
  path: string;
  totalBytes: number;
  receivedBytes: number;
  state: "progress" | "completed" | "failed" | "canceled";
  error?: string;
  startedAt: number;
}

/** 按目录分组的下载列表 */
interface DownloadGroupView {
  dir: string;
  items: DownloadItemView[];
}

interface DownloadsSnapshot {
  dir: string;
  groups: DownloadGroupView[];
}

/** 下载事件触发刷新的最小间隔(ms):同一批事件合并成一次请求 */
const DOWNLOAD_EVENT_REFRESH_MS = 300;

/** 「切到下载视图」请求的生效窗口(ms):开面板事件与该请求到达顺序不定,窗口内以请求为准 */
const VIEW_REQUEST_GRACE_MS = 1000;

/** toast 停留时长(ms) */
const TOAST_MS = 3000;

/** toast 元素上的内部状态(计时器) */
type ToastElement = HTMLDivElement & { _t?: number };

/** 「默认」预设的特殊名(与 Rust `devices::DEFAULT_DEVICE_NAME` 一致;前端写死,
 *  后端据此恢复默认 UA + 内容区回 1100x628) */
const DEVICE_DEFAULT_NAME = "__default__";

/** AI 功能页内的视图(记录 / 下载),同一时刻只显示一个 */
type ViewName = "records" | "downloads";

/** 顶部导航 tab(AI 功能区 / 配置区) */
type TabName = "ai" | "config";

/** DOM 引用集合 */
interface PanelDoms {
  btnSend: HTMLButtonElement;
  btnDevtools: HTMLButtonElement;
  btnPanelClose: HTMLButtonElement;
  deviceSelect: HTMLSelectElement;
  themeSelect: HTMLSelectElement;
  downloadDirInput: HTMLInputElement;
  btnDownloadDirSave: HTMLButtonElement;
  btnDownloadDirReset: HTMLButtonElement;
  btnDownloadDirOpen: HTMLButtonElement;
  downloadDirMsg: HTMLDivElement;
  records: HTMLDivElement;
  dlList: HTMLDivElement;
  dlCount: HTMLSpanElement;
  btnViewRecords: HTMLButtonElement;
  btnViewDownloads: HTMLButtonElement;
  recordsPane: HTMLDivElement;
  downloadsPane: HTMLDivElement;
}

class Panel {
  private doms: PanelDoms;
  private records: AnnotationRecord[] = [];
  /** 面板是否处于打开状态(由 tabs-changed 事件维护;关闭时面板宽度归零) */
  private panelOpen = false;
  /** 上次收到「切到下载视图」请求的时间戳(见 VIEW_REQUEST_GRACE_MS) */
  private downloadsViewRequestAt = 0;
  /** 下载列表在途请求标记(避免 1s 轮询叠加) */
  private dlInFlight = false;
  /** 展开的目录组(组默认收起;轮询重绘时保持展开状态) */
  private expandedDirs = new Set<string>();
  /** 上次渲染的下载快照签名(相同则跳过 DOM 重建) */
  private dlSignature = "";
  /** 上次「事件驱动刷新」的时间戳与节流定时器(见 scheduleDownloadsRefresh) */
  private lastDownloadEventRefreshAt = 0;
  private downloadEventRefreshTimer = 0;
  /** 当前视图(记录 / 下载):下载视图不可见时不轮询 */
  private activeView: ViewName = "records";
  /** 当前顶部 tab(AI 功能区 / 配置区):非 AI 页时下载视图也不可见 */
  private activeTab: TabName = "ai";

  /** 当前生效的下载目录(来自 `/api/download-dir`;用作「打开文件夹」的兜底) */
  private downloadDir = "";

  constructor() {
    this.doms = {
      btnSend: document.getElementById("btn-send") as HTMLButtonElement,
      btnDevtools: document.getElementById("btn-devtools") as HTMLButtonElement,
      btnPanelClose: document.getElementById("btn-panel-close") as HTMLButtonElement,
      deviceSelect: document.getElementById("device-select") as HTMLSelectElement,
      themeSelect: document.getElementById("theme-select") as HTMLSelectElement,
      downloadDirInput: document.getElementById("download-dir-input") as HTMLInputElement,
      btnDownloadDirSave: document.getElementById("btn-download-dir-save") as HTMLButtonElement,
      btnDownloadDirReset: document.getElementById("btn-download-dir-reset") as HTMLButtonElement,
      btnDownloadDirOpen: document.getElementById("btn-download-dir-open") as HTMLButtonElement,
      downloadDirMsg: document.getElementById("download-dir-msg") as HTMLDivElement,
      records: document.getElementById("records") as HTMLDivElement,
      dlList: document.getElementById("dl-list") as HTMLDivElement,
      dlCount: document.getElementById("dl-count") as HTMLSpanElement,
      btnViewRecords: document.getElementById("btn-view-records") as HTMLButtonElement,
      btnViewDownloads: document.getElementById("btn-view-downloads") as HTMLButtonElement,
      recordsPane: document.getElementById("view-records-pane") as HTMLDivElement,
      downloadsPane: document.getElementById("view-downloads-pane") as HTMLDivElement,
    };
    this.bindEvents();
    // 面板可见且处于下载视图时每 1s 拉一次下载列表(见 pollDownloads 的可见性判断)
    window.setInterval(() => {
      void this.pollDownloads();
    }, 1000);
  }

  private bindEvents(): void {
    // 双 tab 切换:AI 功能区 / 配置区(导航样式)
    const tabAi = document.getElementById("tab-ai") as HTMLButtonElement;
    const tabConfig = document.getElementById("tab-config") as HTMLButtonElement;
    this.switchTab("ai");
    tabAi.addEventListener("click", () => this.switchTab("ai"));
    tabConfig.addEventListener("click", () => this.switchTab("config"));
    // 内容区视图切换:记录(默认) / 下载(两者互斥显示,下载视图才轮询)
    this.switchView("records");
    this.doms.btnViewRecords.addEventListener("click", () => this.switchView("records"));
    this.doms.btnViewDownloads.addEventListener("click", () => this.switchView("downloads"));
    // 每次打开面板默认回到 AI 功能页 + 记录视图(tabs-changed 广播面板开关状态)
    void listen<{ panel_open: boolean }>("tabs-changed", (e) => {
      this.panelOpen = !!e.payload?.panel_open;
      if (this.panelOpen) {
        // 「切到下载视图」请求刚到达时保持下载视图(开面板事件与请求事件到达顺序不确定)
        const keepDownloads = Date.now() - this.downloadsViewRequestAt < VIEW_REQUEST_GRACE_MS;
        this.switchView(keepDownloads ? "downloads" : "records");
        this.switchTab("ai");
        // 每次打开面板都取一次最新下载目录(配置页输入框预填值)
        void this.loadDownloadDir();
      }
    });
    // 面板重新可见时立即刷新一次(与 1s 定时器互补)
    document.addEventListener("visibilitychange", () => {
      if (document.visibilityState === "visible" && this.panelOpen) void this.pollDownloads();
    });

    // 底部发送按钮:发送所有记录(批注+截图),结果用统一 toast 通知
    this.doms.btnSend.addEventListener("click", () => {
      // 诊断日志:面板控制台可见(注意 /api/console-logs 读的是"页面"控制台,面板不在此内)
      const imgs = this.records.filter((r) => r.type === "screenshot" && r.image);
      console.log(
        `[send] panel send-all: records=${this.records.length} ids=[${this.records.map((r) => r.index).join(",")}] images=${imgs.length} imageBytes=[${imgs.map((r) => (r.image || "").length).join(",")}]`,
      );
      void invoke<boolean>("panel_cmd", { cmd: "send-all" }).then((sent) => {
        console.log(`[send] panel send-all result: sent=${sent}`);
        if (sent) {
          this.notify("已发送所有记录", "ok");
        } else {
          this.notify("无记录可发送", "bad");
        }
      });
    });
    // 设备切换(选择后应用预设,并关闭面板显示效果)
    // 注意:成功/失败都**不重建选项、不重置 value** —— 下拉保持显示当前所选项(含「默认」)
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
    // 开发者工具开关(用后端返回的真实状态同步按钮激活态,操作后关闭面板)
    this.doms.btnDevtools.addEventListener("click", () => {
      void this.api("/api/devtools", { action: "toggle" })
        .then((resp: { open?: boolean }) => {
          this.doms.btnDevtools.classList.toggle("active", !!resp?.open);
          this.closePanel();
        })
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
    // 下载目录:保存(切换)/ 恢复默认 / 打开文件夹(失败在行内显示服务端原文)
    this.doms.btnDownloadDirSave.addEventListener("click", () => void this.saveDownloadDir());
    this.doms.btnDownloadDirReset.addEventListener("click", () => void this.resetDownloadDir());
    this.doms.btnDownloadDirOpen.addEventListener("click", () => this.openDownloadDir());
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
    // Rust 推送下载开始/状态变化:仅在用户当前处于「下载」视图时刷新列表(不切视图、不展开、不滚动)。
    // 非 Tauri 环境(旧 shell / 纯浏览器打开)没有事件通道,注册失败只告警,不中断面板。
    try {
      void listen<DownloadsChangedPayload>("downloads-changed", (e) => {
        this.onDownloadsChanged(e.payload);
      }).catch((err) => {
        console.warn("[panel] listen downloads-changed failed:", err);
      });
    } catch (err) {
      console.warn("[panel] downloads-changed unavailable:", err);
    }
    // 工具栏下载按钮点击 → Rust 显示面板并广播本事件:面板切到「下载」视图并立即刷新一次
    // (用户主动点击,允许切视图;旧 shell 无此事件时注册失败只告警)
    try {
      void listen("downloads-view-requested", () => {
        this.downloadsViewRequestAt = Date.now();
        this.showDownloadsView();
      }).catch((err) => {
        console.warn("[panel] listen downloads-view-requested failed:", err);
      });
    } catch (err) {
      console.warn("[panel] downloads-view-requested unavailable:", err);
    }
  }

  /** 切换顶部导航 tab(AI 功能区 / 配置区);回到 AI 页且当前是下载视图时补一次刷新 */
  private switchTab(which: TabName): void {
    const tabAi = document.getElementById("tab-ai") as HTMLButtonElement;
    const tabConfig = document.getElementById("tab-config") as HTMLButtonElement;
    const paneAi = document.getElementById("tab-ai-pane") as HTMLDivElement;
    const paneConfig = document.getElementById("tab-config-pane") as HTMLDivElement;
    this.activeTab = which;
    tabAi.classList.toggle("active", which === "ai");
    tabConfig.classList.toggle("active", which === "config");
    paneAi.hidden = which !== "ai";
    paneConfig.hidden = which !== "config";
    if (which === "ai" && this.activeView === "downloads") {
      this.dlSignature = "";
      void this.pollDownloads();
    }
    // 切到配置页时拉一次最新下载目录(输入框始终预填当前生效值)
    if (which === "config") void this.loadDownloadDir();
  }

  /**
   * 切换内容区视图(记录 / 下载)
   *
   * 只显示对应视图(另一个 hidden),同步按钮激活态与 aria-selected;
   * 切到下载视图时强制重绘并立即刷新一次(不必等 1s 轮询)。
   *
   * @param which 目标视图
   */
  private switchView(which: ViewName): void {
    this.activeView = which;
    this.doms.btnViewRecords.classList.toggle("active", which === "records");
    this.doms.btnViewDownloads.classList.toggle("active", which === "downloads");
    this.doms.btnViewRecords.setAttribute("aria-selected", String(which === "records"));
    this.doms.btnViewDownloads.setAttribute("aria-selected", String(which === "downloads"));
    this.doms.recordsPane.hidden = which !== "records";
    this.doms.downloadsPane.hidden = which !== "downloads";
    if (which === "downloads") {
      this.dlSignature = "";
      void this.pollDownloads();
    }
  }

  /**
   * 处理 Rust 的 `downloads-changed` 事件
   *
   * 完全不打扰用户:不改面板可见性、不切视图、不展开分组、不滚动。
   * 仅在用户当前正处于「下载」视图时刷新一次列表(节流 ≥300ms,同批事件合并);
   * 「已下载」提示由工具栏角标负责,面板不再弹 toast。
   * 判定逻辑见 `decideDownloadsChanged`(纯函数,可单测)。
   *
   * @param payload 事件数据
   */
  private onDownloadsChanged(payload: DownloadsChangedPayload | undefined): void {
    const decision = decideDownloadsChanged(payload, this.activeView);
    if (!decision) return;
    if (decision.action !== "refresh") return;
    this.scheduleDownloadsRefresh();
  }

  /** 用户主动点击工具栏下载按钮后切到「下载」视图(切 tab + 视图,并立即刷新列表) */
  private showDownloadsView(): void {
    this.switchTab("ai");
    this.switchView("downloads");
  }

  /**
   * 事件驱动的下载列表刷新(节流 ≥300ms)
   *
   * 窗口内的多余事件不额外发请求,而是在窗口末尾补一次,保证最终状态一定能刷到;
   * 列表不可见时 `pollDownloads` 自身会跳过,不影响用户当前视图。
   */
  private scheduleDownloadsRefresh(): void {
    const wait = DOWNLOAD_EVENT_REFRESH_MS - (Date.now() - this.lastDownloadEventRefreshAt);
    if (wait <= 0) {
      this.lastDownloadEventRefreshAt = Date.now();
      this.dlSignature = "";
      void this.pollDownloads();
      return;
    }
    if (this.downloadEventRefreshTimer) return;
    this.downloadEventRefreshTimer = window.setTimeout(() => {
      this.downloadEventRefreshTimer = 0;
      this.lastDownloadEventRefreshAt = Date.now();
      this.dlSignature = "";
      void this.pollDownloads();
    }, wait);
  }

  /** 调用本地 HTTP 服务 API(带实例鉴权头 `x-bt-token`;客户端见 `api-client.ts`) */
  private async api(path: string, body?: unknown): Promise<any> {
    return postApi(path, body, "panel");
  }

  /** 关闭面板:面板内操作(批注/截图/设备/开发者工具等)执行后统一收起 */
  private closePanel(): void {
    void invoke("toolbar_close_panel");
  }

  /** 统一通知气泡(与批注/截图一致),type: ok|bad|err */
  private notify(message: string, type?: string): void {
    const el = document.getElementById("toast") as ToastElement;
    el.textContent = message;
    el.className = "show " + (["ok", "bad", "err"].includes(type || "") ? type : "ok");
    clearTimeout(el._t);
    el._t = window.setTimeout(() => {
      el.className = "";
    }, TOAST_MS);
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

  /** 拉取下载列表(仅 AI 页的「下载」视图可见时轮询;切到下载视图/重新可见时立即拉一次) */
  private async pollDownloads(): Promise<void> {
    if (
      !this.panelOpen ||
      this.activeTab !== "ai" ||
      this.activeView !== "downloads" ||
      document.visibilityState === "hidden" ||
      this.dlInFlight
    ) {
      return;
    }
    this.dlInFlight = true;
    try {
      const resp = await this.api("/api/downloads", {});
      if (resp?.success && resp.data) this.renderDownloads(resp.data as DownloadsSnapshot);
    } catch (e) {
      console.error("[panel] poll downloads failed:", e);
    } finally {
      this.dlInFlight = false;
    }
  }

  /** 取消进行中的下载(失败用统一 toast 提示) */
  private async cancelDownload(id: string): Promise<void> {
    try {
      const resp = await this.api("/api/downloads", { cancel: id });
      if (resp?.success === false) this.notify(`取消失败:${resp.error || "未知错误"}`, "err");
      else this.notify("已取消下载", "ok");
    } catch (e) {
      console.error("[panel] cancel download failed:", e);
      this.notify("取消失败", "err");
    }
    this.dlSignature = "";
    void this.pollDownloads();
  }

  /** 删除一条已结束的下载(记录 + 磁盘文件;失败用统一 toast 提示) */
  private async deleteDownload(id: string): Promise<void> {
    try {
      const resp = await this.api("/api/downloads", { delete: id });
      if (resp?.success === false) {
        this.notify(`删除失败:${resp.error || "未知错误"}`, "err");
      } else if (resp?.data && resp.data.deleted === false) {
        // 记录已删、文件没删掉(不存在或无权限):如实说明原因
        this.notify(`已删除记录，文件未删除:${resp.data.error || "未知原因"}`, "bad");
      } else {
        this.notify("已删除记录与文件", "ok");
      }
    } catch (e) {
      console.error("[panel] delete download failed:", e);
      this.notify("删除失败", "err");
    }
    this.dlSignature = "";
    void this.pollDownloads();
  }

  /** 拉取最新下载目录:响应含 `dir`(当前)与 `defaultDir`(模板默认) */
  private async loadDownloadDir(): Promise<void> {
    try {
      const resp = await this.api("/api/download-dir", {});
      if (resp?.success === false) {
        this.showDownloadDirMsg(resp.error || "读取下载目录失败", true);
        return;
      }
      const dir = resp?.data?.dir;
      if (typeof dir === "string") {
        this.downloadDir = dir;
        this.doms.downloadDirInput.value = dir;
        this.showDownloadDirMsg("", false);
      }
    } catch (e) {
      console.error("[panel] load download dir failed:", e);
      this.showDownloadDirMsg("读取下载目录失败", true);
    }
  }

  /** 行内提示:失败为小字红色、成功为强调色;`msg` 为空则隐藏 */
  private showDownloadDirMsg(msg: string, isError: boolean): void {
    const el = this.doms.downloadDirMsg;
    el.textContent = msg;
    el.classList.toggle("ok", !!msg && !isError);
    el.hidden = !msg;
  }

  /** 应用目录操作结果:成功就地更新输入框与提示;失败显示服务端错误原文 */
  private applyDownloadDirResult(resp: any, okText: string): void {
    if (resp?.success === false) {
      this.showDownloadDirMsg(resp.error || "操作失败", true);
      return;
    }
    const dir = resp?.data?.dir;
    if (typeof dir === "string") {
      this.downloadDir = dir;
      this.doms.downloadDirInput.value = dir;
    }
    this.showDownloadDirMsg(okText, false);
  }

  /** 保存下载目录(`POST /api/download-dir {set}`;校验失败把错误原文显示在行内) */
  private async saveDownloadDir(): Promise<void> {
    const raw = this.doms.downloadDirInput.value.trim();
    if (!raw) {
      this.showDownloadDirMsg("目录不能为空", true);
      return;
    }
    try {
      const resp = await this.api("/api/download-dir", { set: raw });
      this.applyDownloadDirResult(resp, "已保存");
    } catch (e) {
      console.error("[panel] set download dir failed:", e);
      this.showDownloadDirMsg("保存失败", true);
    }
  }

  /** 恢复默认下载目录(`{reset:true}`:删持久化文件并切回模板默认目录) */
  private async resetDownloadDir(): Promise<void> {
    try {
      const resp = await this.api("/api/download-dir", { reset: true });
      this.applyDownloadDirResult(resp, "已恢复默认");
    } catch (e) {
      console.error("[panel] reset download dir failed:", e);
      this.showDownloadDirMsg("恢复默认失败", true);
    }
  }

  /** 用系统文件管理器打开输入框里的目录(为空时回退到最近一次生效目录) */
  private openDownloadDir(): void {
    const path = this.doms.downloadDirInput.value.trim() || this.downloadDir;
    if (!path) {
      this.showDownloadDirMsg("目录不能为空", true);
      return;
    }
    void invoke("panel_open_download_dir", { path }).catch((e) => {
      console.error("[panel] open download dir failed:", e);
      this.showDownloadDirMsg("打开文件夹失败", true);
    });
  }

  /** 渲染下载分组(按目录分组、默认收起;组内最新在前) */
  private renderDownloads(data: DownloadsSnapshot): void {
    const groups = Array.isArray(data?.groups) ? data.groups : [];
    const total = groups.reduce((n, g) => n + (Array.isArray(g.items) ? g.items.length : 0), 0);
    this.doms.dlCount.textContent = String(total);
    // 展开状态也进签名:用户手动展开/收起时需要重绘,不能命中「数据未变」短路
    const expanded = [...this.expandedDirs].sort();
    const signature = JSON.stringify({ dir: data?.dir, groups, expanded });
    if (signature === this.dlSignature && this.doms.dlList.childElementCount > 0) return;
    this.dlSignature = signature;
    const list = this.doms.dlList;
    const scrollTop = list.scrollTop;
    list.innerHTML = "";
    if (groups.length === 0) {
      const empty = document.createElement("div");
      empty.className = "dl-empty";
      empty.textContent = "暂无下载";
      list.appendChild(empty);
      list.scrollTop = 0;
      return;
    }
    for (const group of groups) list.appendChild(this.renderDownloadGroup(group));
    // 行内 trash 图标是动态创建的,创建后单独渲染一次(只扫列表内的图标)
    createIcons({ icons: { Trash2 }, root: list });
    // 重绘后恢复用户原来的滚动位置(刷新不改变用户的浏览位置)
    list.scrollTop = scrollTop;
  }

  /** 单个目录组(点击组标题展开/收起) */
  private renderDownloadGroup(group: DownloadGroupView): HTMLElement {
    const open = this.expandedDirs.has(group.dir);
    const wrap = document.createElement("div");
    wrap.className = "dl-group" + (open ? " open" : "");
    const head = document.createElement("button");
    head.className = "dl-group-head";
    head.title = group.dir;
    head.type = "button";
    const caret = document.createElement("span");
    caret.className = "dl-caret";
    caret.textContent = "▶";
    const name = document.createElement("span");
    name.className = "dl-dir";
    name.textContent = dirTail(group.dir);
    const num = document.createElement("span");
    num.className = "dl-num";
    num.textContent = String(group.items.length);
    head.append(caret, name, num);
    const body = document.createElement("div");
    body.className = "dl-group-body";
    body.hidden = !open;
    head.addEventListener("click", () => {
      const nowOpen = !this.expandedDirs.has(group.dir);
      if (nowOpen) this.expandedDirs.add(group.dir);
      else this.expandedDirs.delete(group.dir);
      wrap.classList.toggle("open", nowOpen);
      body.hidden = !nowOpen;
    });
    for (const item of group.items) body.appendChild(this.renderDownloadRow(item, group.dir));
    wrap.append(head, body);
    return wrap;
  }

  /** 单条下载:文件名、进度条(未知总量为不确定态)、大小、状态、来源 host + 操作 */
  private renderDownloadRow(item: DownloadItemView, dir: string): HTMLElement {
    const row = document.createElement("div");
    row.className = "dl-row";
    const top = document.createElement("div");
    top.className = "dl-row-top";
    const name = document.createElement("span");
    name.className = "dl-name";
    name.textContent = item.filename;
    name.title = item.path;
    const size = document.createElement("span");
    size.className = "dl-size";
    size.textContent = downloadSizeLabel(item);
    top.append(name, size);

    const bar = document.createElement("div");
    bar.className = "dl-bar";
    const fill = document.createElement("div");
    fill.className = "dl-bar-fill";
    if (item.state === "progress" && item.totalBytes <= 0) {
      // 总量未知:不确定态动画,不假装百分比
      bar.classList.add("indeterminate");
    } else {
      const percent =
        item.totalBytes > 0
          ? Math.min(100, Math.round((item.receivedBytes / item.totalBytes) * 100))
          : item.state === "completed"
            ? 100
            : 0;
      fill.style.width = `${percent}%`;
    }
    if (item.state === "completed") bar.classList.add("done");
    if (item.state === "failed") bar.classList.add("failed");
    bar.appendChild(fill);

    const meta = document.createElement("div");
    meta.className = "dl-meta";
    const state = document.createElement("span");
    state.className = "dl-state" + (item.state === "failed" ? " failed" : "");
    state.textContent = DOWNLOAD_STATE_LABEL[item.state] || item.state;
    const host = document.createElement("span");
    host.className = "dl-host";
    host.textContent = hostOf(item.url);
    if (item.state === "failed" && item.error) host.title = item.error;
    meta.append(state, host);

    const actions = document.createElement("div");
    actions.className = "dl-actions";
    if (item.state === "progress") {
      const cancel = document.createElement("button");
      cancel.type = "button";
      cancel.textContent = "取消";
      cancel.addEventListener("click", () => {
        cancel.disabled = true;
        void this.cancelDownload(item.id);
      });
      actions.appendChild(cancel);
    }
    const openDir = document.createElement("button");
    openDir.type = "button";
    openDir.textContent = "打开文件夹";
    openDir.addEventListener("click", () => {
      void invoke("panel_open_download_dir", { path: dir }).catch((e) =>
        console.error("[panel] open download dir failed:", e),
      );
    });
    actions.appendChild(openDir);
    actions.appendChild(this.renderDeleteButton(item));

    row.append(top, bar, meta, actions);
    return row;
  }

  /**
   * 单行「删除」按钮(trash 图标 + 文字;窄屏 CSS 只留图标,`title` 保可读)
   *
   * 进行中的记录禁用删除(先取消),`title` 明确提示原因。
   *
   * @param item 该行下载记录
   * @returns 已绑定点击行为的按钮
   */
  private renderDeleteButton(item: DownloadItemView): HTMLButtonElement {
    const inProgress = item.state === "progress";
    const btn = document.createElement("button");
    btn.type = "button";
    btn.className = "dl-del";
    btn.disabled = inProgress;
    const title = inProgress ? "正在下载，请先取消再删除" : `删除记录与文件：${item.filename}`;
    btn.title = title;
    btn.setAttribute("aria-label", title);
    const icon = document.createElement("i");
    icon.setAttribute("data-lucide", "trash-2");
    icon.setAttribute("aria-hidden", "true");
    const label = document.createElement("span");
    label.className = "dl-del-label";
    label.textContent = "删除";
    btn.append(icon, label);
    btn.addEventListener("click", () => {
      btn.disabled = true;
      void this.deleteDownload(item.id);
    });
    return btn;
  }
  /** 主动拉取记录/模式状态/设备列表(启动时) */
  async init(): Promise<void> {
    // 面板初始可见性(面板 webview 常驻,打开状态由 Rust 决定;默认记录视图,不拉下载)
    try {
      const st = await invoke<{ panel_open: boolean }>("toolbar_state");
      this.panelOpen = !!st?.panel_open;
    } catch (e) {
      console.error("[panel] load panel state failed:", e);
    }
    try {
      this.records = await invoke<AnnotationRecord[]>("panel_records");
      this.render();
    } catch (e) {
      console.error("[panel] init failed:", e);
    }
    // 加载设备列表(首项「默认」写死,其余项来自 /api/device/list)
    try {
      const r = await this.api("/api/device/list");
      const devices = r.data?.devices || [];
      this.doms.deviceSelect.innerHTML = "";
      const def = document.createElement("option");
      def.value = DEVICE_DEFAULT_NAME;
      def.textContent = "默认";
      this.doms.deviceSelect.appendChild(def);
      for (const d of devices) {
        const opt = document.createElement("option");
        opt.value = d.name;
        opt.textContent = `${d.name} (${d.width}x${d.height})`;
        this.doms.deviceSelect.appendChild(opt);
      }
      // 初始选中「默认」(当前无预设 = 默认 UA/默认尺寸);此后 change 只改 value,不重建 → 保持所选
      this.doms.deviceSelect.value = DEVICE_DEFAULT_NAME;
    } catch (e) {
      console.error("[panel] load devices failed:", e);
    }
    // 预填配置页的下载目录(打开面板/切到配置页时还会再拉一次最新值)
    await this.loadDownloadDir();
  }
}

const panel = new Panel();
void panel.init();
void initTheme();
// lucide 图标替换(tab/视图导航/开发工具/批注/截图/发送/关闭/下载目录)
createIcons({
  icons: { Bot, Settings, Wrench, PenLine, Camera, Send, X, List, Download, Save, RotateCcw, FolderOpen },
});
console.log("[panel] ready");

/** 截图 base64 转可显示 data URL(裸 base64 需补前缀) */
function toDataUrl(base64: string): string {
  return base64.startsWith("data:") ? base64 : `data:image/png;base64,${base64}`;
}
/** 下载状态中文文案 */
const DOWNLOAD_STATE_LABEL: Record<string, string> = {
  progress: "下载中",
  completed: "已完成",
  failed: "失败",
  canceled: "已取消",
};

/** 目录尾段(组标题);根路径保底显示原串 */
function dirTail(dir: string): string {
  const parts = dir.replace(/[\\/]+$/, "").split(/[\\/]/);
  return parts[parts.length - 1] || dir;
}

/** 来源主机(解析失败时留空,不显示原始 URL) */
function hostOf(url: string): string {
  try {
    return new URL(url).host;
  } catch {
    return "";
  }
}

/** 字节数 → 可读大小 */
function formatBytes(bytes: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value >= 10 || unit === 0 ? Math.round(value) : value.toFixed(1)} ${units[unit]}`;
}

/** 大小列:已收 / 总量(总量未知时显示 `?`) */
function downloadSizeLabel(item: DownloadItemView): string {
  const received = formatBytes(item.receivedBytes);
  if (item.totalBytes > 0) return `${received} / ${formatBytes(item.totalBytes)}`;
  return received;
}
