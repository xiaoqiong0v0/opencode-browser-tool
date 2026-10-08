import { spawn } from "child_process";
import { resolve, dirname } from "path";
import { existsSync } from "fs";
import { fileURLToPath } from "url";
import { log } from "./logger.js";
import { getNotifyUrl } from "./notify-server.js";

import { cachedBinaryPath, describeBinaryStatus, ensureBinary } from "./binary.js";

const __dirname = dirname(fileURLToPath(import.meta.url));

/** 就绪轮询超时(ms) */
const READY_TIMEOUT_MS = 20000;
/** 就绪轮询间隔(ms) */
const READY_POLL_INTERVAL_MS = 300;

/** 二进制来源(用于日志一眼判断用的是哪份) */
type ShellSource = "env" | "local-dev" | "cargo-target-dir" | "download-cache";

/** opencode 应用/项目目录(插件初始化与工具上下文提供);用于定位本地 Rust 开发构建 */
let appDirectory = "";

/** 记录 opencode 目录(插件初始化 / 工具调用时调用) */
export function setAppDirectory(dir: string): void {
  if (dir) appDirectory = dir;
}

// Rust 二进制查找:优先级 BT_SHELL_PATH → 本地开发构建(含 opencode 应用目录)→ 下载缓存(binary.ts)
// 注意:用 `.tmp/publish-local.ps1` 覆盖到 ~/.cache/opencode/packages/... 后,插件 dist 的相对路径
// (`<install>/../rust/...`)已指向缓存目录而非仓库 → 必须借助 opencode 传入的 directory 定位本地构建。
// 导出仅供自验/单测直接调用。
export function resolveShellBinary(): { path: string; source: ShellSource } {
  const platform = process.platform === "win32" ? "win-x64" : "linux-x64";
  const bin = process.platform === "win32" ? "bt-shell.exe" : "bt-shell";
  // 1) 显式指定:最优先(即使文件暂不存在也返回,由 startService 给出明确错误)
  const envPath = process.env.BT_SHELL_PATH;
  if (envPath) return { path: envPath, source: "env" };
  const cargoTargetDir = process.env.CARGO_TARGET_DIR;
  // 2) 本地开发构建优先(开发者/源码树):含 opencode 应用目录的多种可能布局
  const candidates: Array<[string | undefined, ShellSource]> = [];
  if (appDirectory) {
    candidates.push(
      [resolve(appDirectory, "rust", "target", "release", bin), "local-dev"], // dir = 仓库根
      [resolve(appDirectory, "target", "release", bin), "local-dev"], // dir = rust/
      [resolve(appDirectory, "..", "rust", "target", "release", bin), "local-dev"], // dir = 仓库子目录
      [resolve(appDirectory, "rust", "target", "debug", bin), "local-dev"],
      [resolve(appDirectory, "target", "debug", bin), "local-dev"],
      [resolve(appDirectory, "..", "rust", "target", "debug", bin), "local-dev"],
    );
  }
  candidates.push(
    // 3) 源码树开发(插件直接从仓库 dist 加载时):<install>/../rust/...
    [resolve(__dirname, "..", "rust", "target", "release", bin), "local-dev"],
    [resolve(__dirname, "..", "rust", "target", "debug", bin), "local-dev"],
    // 4) 自定义 CARGO_TARGET_DIR(WSL 内构建时)
    [cargoTargetDir ? resolve(cargoTargetDir, "release", bin) : undefined, "cargo-target-dir"],
    [cargoTargetDir ? resolve(cargoTargetDir, "debug", bin) : undefined, "cargo-target-dir"],
    [resolve(__dirname, "bin", `bt-shell-${platform}.exe`), "local-dev"],
    [resolve(__dirname, "bin", `bt-shell-${platform}`), "local-dev"],
  );
  for (const [p, source] of candidates) {
    if (p && existsSync(p)) return { path: p, source };
  }
  // 5) 下载缓存(未下载时由 startService 先 ensureBinary)
  return { path: cachedBinaryPath(), source: "download-cache" };
}

/** 解析附着模式目标地址:BT_SHELL_URL 优先,其次 BT_SHELL_PORT;均未设置返回 null */
function resolveAttachBaseUrl(): string | null {
  const url = process.env.BT_SHELL_URL?.trim();
  if (url) return url.replace(/\/+$/, "");
  const port = process.env.BT_SHELL_PORT?.trim();
  if (port) return `http://127.0.0.1:${port}`;
  return null;
}

let serviceProcess: any = null;
/** 完整基地址(启动模式 http://127.0.0.1:<port>,附着模式 BT_SHELL_URL) */
let serviceBaseUrl = "";
let serviceReady = false;
/** 附着模式:连接外部已运行 shell,不 spawn 也不 kill */
let attachMode = false;
/** 服务启动参数(configureService 保存,ensureService 懒启动时使用) */
let serviceConfig: { nodePath: string; browsersPath?: string; sessionIsolation?: boolean; browserType?: string; userDataDir?: string } | null = null;
/** 正在启动中的 Promise(并发保护,多个工具同时调用只启动一次) */
let starting: Promise<void> | null = null;

/** 保存服务启动参数(插件加载时调用,不立即启动窗口) */
export function configureService(opts: { nodePath: string; browsersPath?: string; sessionIsolation?: boolean; browserType?: string; userDataDir?: string }): void {
  serviceConfig = opts;
}

/** 运行时切换用户数据目录(下次 ensureService 重启时生效) */
export function setUserDataDir(dir: string): void {
  if (serviceConfig) serviceConfig.userDataDir = dir;
}

/** 服务就绪回调(插件注册:做一次初始 drain) */
let serviceReadyHandler: (() => void) | null = null;

/** 注册服务就绪回调(spawn/附着成功且注册 notify-url 之后调用一次) */
export function setServiceReadyHandler(cb: () => void): void {
  serviceReadyHandler = cb;
}

/** 懒启动服务:未启动则 spawn bt-shell(弹窗);已启动直接返回;并发时复用同一个 Promise */
export async function ensureService(): Promise<void> {
  if (serviceReady) return;
  if (starting) return starting;

  // 附着模式:连接外部已运行的 shell,不 spawn
  const attachUrl = resolveAttachBaseUrl();
  if (attachUrl) {
    starting = attachToService(attachUrl).finally(() => {
      starting = null;
    });
  } else {
    // 启动模式:懒启动本地 shell
    const cfg = serviceConfig;
    if (!cfg) throw new Error("Service not configured");
    starting = startService(cfg.nodePath, cfg.browsersPath, cfg.sessionIsolation, cfg.browserType, cfg.userDataDir).finally(() => {
      starting = null;
    });
  }
  await starting;
  // 服务就绪:把插件通知地址注册给 shell(推送式投递;附着模式同样走这里)
  await registerNotifyUrl();
  try {
    serviceReadyHandler?.();
  } catch (e) {
    log.error("[notify] service-ready handler failed", e as Error);
  }
}

/** 把插件通知地址注册给 shell(/api/notify-url);直接 fetch,避免 callApi 触发 ensureService 递归 */
async function registerNotifyUrl(): Promise<void> {
  const url = getNotifyUrl();
  if (!url || !serviceBaseUrl) return;
  try {
    const res = await fetch(`${serviceBaseUrl}/api/notify-url`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ url }),
    });
    if (res.ok) log.info(`[notify] notify-url registered: ${url}`);
    else log.error(`[notify] register failed: HTTP ${res.status}`);
  } catch (e) {
    log.error("[notify] register failed", e as Error);
  }
}

async function callApi(path: string, body?: any, sessionId?: string): Promise<any> {
  // 懒启动:首次调用工具时自动打开窗口
  await ensureService();
  const payload = { ...(body || {}), _sessionId: sessionId || "" };
  const res = await fetch(`${serviceBaseUrl}${path}`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(payload),
  });
  const json = await res.json();
  if (!json.success) throw new Error(json.error || "Service error");
  return json.data;
}

/** 轮询等待服务就绪(READY_TIMEOUT_MS 超时);超时抛出给定提示 */
async function waitForReady(baseUrl: string, timeoutMessage: string): Promise<void> {
  const deadline = Date.now() + READY_TIMEOUT_MS;
  while (Date.now() < deadline) {
    try {
      const res = await fetch(`${baseUrl}/api/status`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: "{}",
      });
      if (res.ok) return;
    } catch {
      // 服务未启动,继续等待
    }
    await new Promise((r) => setTimeout(r, READY_POLL_INTERVAL_MS));
  }
  throw new Error(timeoutMessage);
}

/** 附着到外部已运行的 shell(不 spawn):等待就绪后记录 base URL */
async function attachToService(baseUrl: string): Promise<void> {
  attachMode = true;
  log.info(`Attaching to existing bt-shell at ${baseUrl} (no spawn)`);
  await waitForReady(
    baseUrl,
    `External bt-shell not ready at ${baseUrl}. Check BT_SHELL_URL/BT_SHELL_PORT and that the shell is running.`,
  );
  serviceBaseUrl = baseUrl;
  serviceReady = true;
  log.info(`Attached to existing bt-shell at ${baseUrl}`);
}

export async function startService(
  nodePath: string,
  browsersPath?: string,
  sessionIsolation?: boolean,
  browserType?: string,
  userDataDir?: string,
): Promise<void> {
  attachMode = false;
  const resolved = resolveShellBinary();
  let shell = resolved.path;
  // 未就绪:先(有界)等待下载完成;失败/超时给出明确提示
  if (!existsSync(shell)) {
    if (process.env.BT_SHELL_PATH) {
      throw new Error(`BT_SHELL_PATH 指向的二进制不存在：${shell}`);
    }
    const st = await ensureBinary(20000);
    if (st.state !== "ready" || !st.path) {
      throw new Error(describeBinaryStatus(st));
    }
    shell = st.path;
  }
  // 记录最终选中的二进制与来源,便于一眼判断用的是哪份(local-dev / env / download-cache …)
  log.info(`Shell binary: ${shell} (source=${resolved.source})`);
  // 显式指定端口(release 模式 GUI 程序无控制台,无法解析 stdout)
  const port = 18000 + Math.floor(Math.random() * 1000);
  const args = ["--browsers-path", browsersPath || "", "--port", String(port)];
  if (browserType) args.push("--browser", browserType);
  if (sessionIsolation) args.push("--session-isolation");
  // 多用户配置:userDataDir 指向独立 WebView2 用户数据目录
  if (userDataDir) args.push("--user-data-dir", userDataDir);
  // 推送式投递:把插件本地通知地址传给 shell(亦可用 BT_SHELL_NOTIFY_URL;附着模式用 /api/notify-url 注册)
  const notifyUrl = getNotifyUrl();
  if (notifyUrl) args.push("--notify-url", notifyUrl);
  serviceProcess = spawn(shell, args, { stdio: ["ignore", "pipe", "pipe"] });
  // 把 shell 的 stdout/stderr 转发进共享日志:Rust 侧 println!/eprintln! 默认被管道吞掉(无人读取 → 日志永远看不到)
  const forwardShell = (tag: string) => (chunk: Buffer) => {
    for (const line of chunk.toString().split(/\r?\n/)) {
      const s = line.trim();
      if (s) log.info(`${tag} ${s}`);
    }
  };
  serviceProcess.stdout?.on("data", forwardShell("[shell:out]"));
  serviceProcess.stderr?.on("data", forwardShell("[shell:err]"));

  // 等待服务就绪(轮询端口)
  const baseUrl = `http://127.0.0.1:${port}`;
  await waitForReady(baseUrl, "Service timeout: bt-shell did not start. Build it with: cd rust && cargo build --release");
  serviceBaseUrl = baseUrl;
  serviceReady = true;
  log.info(`Shell service ready on port ${port} (${shell})`);
}

/** 等待子进程退出(有界超时);已退出/出错返回 true,超时返回 false */
function waitForExit(proc: any, timeoutMs: number): Promise<boolean> {
  return new Promise<boolean>((resolveExit) => {
    if (!proc || proc.exitCode !== null || proc.signalCode !== null) return resolveExit(true);
    let done = false;
    const finish = (v: boolean) => {
      if (done) return;
      done = true;
      clearTimeout(timer);
      resolveExit(v);
    };
    const timer = setTimeout(() => finish(false), timeoutMs);
    proc.once("exit", () => finish(true));
    proc.once("error", () => finish(true));
  });
}

export async function stopService(): Promise<void> {
  if (attachMode) {
    // 附着模式:外部 shell 进程不归插件管,**绝不 kill**,只清空本地状态
    log.info("Detached from external bt-shell (process left running)");
    serviceReady = false;
    serviceBaseUrl = "";
    attachMode = false;
    return;
  }
  const proc = serviceProcess;
  if (proc) {
    // 先等 shell 自己退出(Rust /api/close 已 app.exit(0) 优雅退出);有界 5s,仍存活才按该 PID 补 SIGTERM
    const exited = await waitForExit(proc, 5000);
    if (exited) {
      log.info("bt-shell exited gracefully");
    } else {
      log.info(`bt-shell did not exit within 5s; sending SIGTERM to pid=${proc.pid}`);
      try { proc.kill("SIGTERM"); } catch {}
    }
    serviceProcess = null;
  }
  serviceReady = false;
  serviceBaseUrl = "";
  attachMode = false;
}

/** 打开浏览器窗口(未启动则启动;已启动幂等) */
export async function openWindow(): Promise<void> {
  await ensureService();
}

/** 服务当前是否运行 */
export function isRunning(): boolean {
  return serviceReady;
}

/**
 * 纯查询：探测 shell 是否在运行，**绝不 spawn**。
 * 仅在本进程已启动/附着（`serviceBaseUrl` 已知）时做一次短超时 HTTP 探测；否则返回 null。
 * 用于 `get_browser_status` 这类只读命令，避免查询也弹出浏览器窗口。
 */
export async function probeStatus(timeoutMs = 1500): Promise<any | null> {
  if (!serviceReady || !serviceBaseUrl) return null;
  const ac = new AbortController();
  const timer = setTimeout(() => ac.abort(), timeoutMs);
  try {
    const res = await fetch(`${serviceBaseUrl}/api/status`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: "{}",
      signal: ac.signal,
    });
    if (!res.ok) return null;
    const json = await res.json();
    return json?.success ? json.data : null;
  } catch {
    return null;
  } finally {
    clearTimeout(timer);
  }
}

function cmd(name: string) {
  return (params?: any, sessionId?: string) => callApi(`/api/${name}`, params, sessionId);
}

export const service = {
  navigate: cmd("navigate"),
  click: cmd("click"),
  fill: cmd("fill"),
  clear: cmd("clear"),
  select: cmd("select"),
  hover: cmd("hover"),
  pressKey: cmd("press-key"),
  drag: cmd("drag"),
  uploadFile: cmd("upload-file"),
  screenshot: cmd("screenshot"),
  evaluate: cmd("evaluate"),
  visibleText: cmd("visible-text"),
  visibleHtml: cmd("visible-html"),
  elementState: cmd("element-state"),
  dropdownOptions: cmd("dropdown-options"),
  waitForSelector: cmd("wait-for-selector"),
  scroll: cmd("scroll"),
  scrollToElement: cmd("scroll-to-element"),
  reload: cmd("reload"),
  goBack: cmd("go-back"),
  goForward: cmd("go-forward"),
  resize: cmd("resize"),
  device: cmd("device"),
  deviceList: cmd("device/list"),
  devtools: cmd("devtools"),
  mediaMode: cmd("media/mode"),
  mediaAudio: cmd("media/audio"),
  mediaVideo: cmd("media/video"),
  close: cmd("close"),
  newTab: cmd("tabs/new"),
  switchTab: cmd("tabs/switch"),
  closeTab: cmd("tabs/close"),
  clickSwitchTab: cmd("click-switch-tab"),
  iframeClick: cmd("iframe-click"),
  iframeFill: cmd("iframe-fill"),
  userAgent: cmd("user-agent"),
  accessibility: cmd("accessibility"),
  notify: cmd("notify"),
  consoleLogs: cmd("console-logs"),
  expectResponse: cmd("expect-response"),
  assertResponse: cmd("assert-response"),
  panelConfig: cmd("panel-config"),
  status: cmd("status"),
  tabs: cmd("tabs"),
  annotateToggle: cmd("annotate/toggle"),
  annotateRecords: cmd("annotate/records"),
  annotateSend: cmd("annotate/send"),
  annotateConsumeSent: cmd("annotate/consume-sent"),
};

