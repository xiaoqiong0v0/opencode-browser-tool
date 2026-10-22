// bt-shell 客户端:每会话独立 shell 实例(1.9.0 会话隔离架构)
//
// 架构:实例注册表 Map<sid, ShellInstance> —— 同一 opencode 进程内的不同会话各持有
// 自己的 bt-shell 进程(独立端口 + 独立 --user-data-dir + 独立窗口标题),互不共享
// 窗口/标签/UA/批注等一切 shell 状态;进程回收只按注册表里的 PID,**绝不按名杀进程**。
// 附着模式(BT_SHELL_URL/BT_SHELL_PORT)例外:连接外部单例,放弃会话隔离(见 docs/implementation.md)。
import { spawn } from "child_process";
import { resolve, dirname } from "path";
import { existsSync } from "fs";
import { fileURLToPath } from "url";
import { log } from "./logger.js";
import { getNotifyUrlForSession } from "./notify-server.js";
import { tf } from "./i18n/index.js";

import { cachedBinaryPath, describeBinaryStatus, ensureBinary } from "./binary.js";

const __dirname = dirname(fileURLToPath(import.meta.url));

/** 就绪轮询超时(ms) */
const READY_TIMEOUT_MS = 20000;
/** 就绪轮询间隔(ms) */
const READY_POLL_INTERVAL_MS = 300;
/** 端口撞车/启动失败时的重试次数(每次换一个随机端口) */
const PORT_START_ATTEMPTS = 5;
/** 会话标题解析上限(ms):超时就用 sid 后缀,绝不因标题卡住启动 */
const TITLE_RESOLVE_TIMEOUT_MS = 250;

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
  // 1) 显式指定:最优先(即使文件暂不存在也返回,由启动链路 resolveShellPath 给出明确错误)
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
  // 5) 下载缓存(未下载时由 resolveShellPath 先 ensureBinary)
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

/** 一个会话的 shell 实例(进程 + 端点 + 就绪态) */
interface ShellInstance {
  /** opencode 会话 id(注册表键;附着模式下为首次请求的 sid) */
  sid: string;
  /** 子进程句柄(附着模式恒 null —— 外部进程不归插件管) */
  proc: any;
  /** HTTP 端点,如 http://127.0.0.1:18234 */
  baseUrl: string;
  /** 端口(诊断用) */
  port: number;
  /** 该实例的 --user-data-dir(诊断/取证用;未设置则为 "") */
  dataDir: string;
  /** 窗口标题(诊断/取证用) */
  title: string;
  ready: boolean;
  /** 附着模式(外部单例,不 spawn 不 kill) */
  attach: boolean;
}

/** 实例注册表:每会话一个 shell;并发启动保护见 startingSid */
const sessions = new Map<string, ShellInstance>();
/** 正在启动中的会话 → Promise(并发保护:同一 sid 多个工具同时调用只启动一次) */
const startingSid = new Map<string, Promise<ShellInstance>>();
/** 已请求关闭的会话(P3-①):close 与启动并发时置位,启动链完成后据此自毁,避免孤儿窗口 */
const closedSids = new Set<string>();
/** 附着模式共享实例(外部 shell;所有 sid 共用,放弃会话隔离) */
let attachInstance: ShellInstance | null = null;
/**
 * 附着模式下**已注册过** notify-url 的 `baseUrl|sid`(去重,避免重复 POST)
 *
 * 语义:`/api/notify-url` 在 shell 侧是**全局单值**(见 rust/src/service.rs 的 set_notify_url),
 * 待发送队列(state.sent_records)也是**全局唯一**的。附着模式只共享一个 shell ⇒ 队列全局,
 * drain 由 notify URL 里带的 sid 执行(后注册的 sid 覆盖前一个,与「按 sid 分发」不同)。
 */
const attachNotifyRegistered = new Set<string>();

/** 服务启动参数(configureService 保存,懒启动时使用) */
let serviceConfig: {
  nodePath: string;
  browsersPath?: string;
  /** 由调用方(插件层)解析某会话的 user-data-dir;不提供则不传 --user-data-dir */
  resolveDataDir?: (sid: string) => string;
} | null = null;

/** 会话 id 兜底解析器(钩子缓存):工具调用没带 _sessionId 时用它 */
let sessionIdResolver: (() => string) | null = null;
/** 可选会话标题解析器(client.session.get 之类);拿不到/超时回退 sid 后缀,不阻塞启动 */
let titleResolver: ((sid: string) => Promise<string | null>) | null = null;
/** 实例就绪回调(插件注册:对该 sid 做一次初始 drain) */
let serviceReadyHandler: ((sid: string) => void) | null = null;

/** 保存服务启动参数(插件加载时调用,不立即启动窗口) */
export function configureService(opts: {
  nodePath: string;
  browsersPath?: string;
  resolveDataDir?: (sid: string) => string;
}): void {
  serviceConfig = opts;
}

/** 注册会话 id 兜底解析器(通常指向插件的钩子缓存 sessionCache.get) */
export function setSessionIdResolver(fn: () => string): void {
  sessionIdResolver = fn;
}

/** 注册可选的会话标题解析器(返回 null/抛错则用 sid 后缀;有 250ms 上限,不阻塞启动) */
export function setTitleResolver(fn: (sid: string) => Promise<string | null>): void {
  titleResolver = fn;
}

/** 注册实例就绪回调(spawn/附着成功且注册 notify-url 之后,按 sid 调用一次) */
export function setServiceReadyHandler(cb: (sid: string) => void): void {
  serviceReadyHandler = cb;
}

/**
 * 解析本次调用的会话 id:显式参数 > 请求体 _sessionId > 钩子缓存兜底 > "default" 桶
 * "default" 桶用于拿不到任何 sid 的场景(仍有独立实例,不会与其它会话互踩)
 */
export function resolveSessionId(explicit?: string): string {
  const s = (explicit || "").trim();
  if (s) return s;
  try {
    const cached = sessionIdResolver?.() || "";
    if (cached.trim()) return cached.trim();
  } catch {
    // 解析器异常不阻断调用
  }
  return "default";
}

/** 该会话实例是否就绪(不给 sid 则任一实例就绪即 true) */
export function isRunning(sid?: string): boolean {
  if (sid) {
    const s = resolveSessionId(sid);
    if (sessions.get(s)?.ready) return true;
    return !!attachInstance?.ready;
  }
  for (const inst of sessions.values()) if (inst.ready) return true;
  return !!attachInstance?.ready;
}

/** 组装窗口标题:优先会话标题(有界等待),拿不到用 sid 后 6 位,保证任务栏可分辨 */
async function resolveWindowTitle(sid: string): Promise<string> {
  const suffix = sid.length > 6 ? sid.slice(-6) : sid || "main";
  const fallback = `bt-shell · ${suffix}`;
  if (!titleResolver) return fallback;
  try {
    const t = await Promise.race([
      Promise.resolve().then(() => titleResolver!(sid)),
      new Promise<string | null>((r) => setTimeout(() => r(null), TITLE_RESOLVE_TIMEOUT_MS)),
    ]);
    if (t && t.trim()) return `bt-shell · ${t.trim()}`;
  } catch {
    // 标题拿不到就用后缀,绝不因此失败
  }
  return fallback;
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

/**
 * 终止**本插件自己 spawn 的**子进程(先等优雅退出,超时才 SIGTERM 该句柄/PID)。
 * 红线:只允许杀自己 spawn 的子进程、按句柄/PID —— 绝不按名/通配匹配。
 */
async function terminateOwnProcess(proc: any): Promise<void> {
  if (!proc) return;
  const exited = await waitForExit(proc, 5000);
  if (exited) {
    log.info(`bt-shell pid=${proc.pid} exited gracefully`);
    return;
  }
  log.info(`bt-shell pid=${proc.pid} did not exit within 5s; sending SIGTERM to that pid`);
  try {
    proc.kill("SIGTERM");
  } catch {}
  await waitForExit(proc, 2000);
}

/** 把 shell 的 stdout/stderr 转发进共享日志(Rust println! 默认会被管道吞掉) */
function attachStdio(proc: any, sid: string): void {
  const forwardShell = (tag: string) => (chunk: Buffer) => {
    for (const line of chunk.toString().split(/\r?\n/)) {
      const s = line.trim();
      if (s) log.info(`[${tag}][sid=${sid}] ${s}`);
    }
  };
  proc.stdout?.on("data", forwardShell("shell:out"));
  proc.stderr?.on("data", forwardShell("shell:err"));
}

/** 二进制就绪性检查(不存在则等下载;失败抛明确错误) */
async function resolveShellPath(): Promise<string> {
  const resolved = resolveShellBinary();
  let shell = resolved.path;
  if (!existsSync(shell)) {
    if (process.env.BT_SHELL_PATH) {
      // 只提示环境变量名,不回流绝对路径(路径已在插件日志)
      throw new Error(tf("client.bt_shell_path_missing"));
    }
    const st = await ensureBinary(20000);
    if (st.state !== "ready" || !st.path) {
      throw new Error(describeBinaryStatus(st));
    }
    shell = st.path;
  }
  log.info(`Shell binary: ${shell} (source=${resolved.source})`);
  return shell;
}

/**
 * spawn 一个新 shell 实例(带端口重试):端口撞车/启动超时 → 只杀掉**本次刚 spawn 的**
 * 子进程(按句柄/PID),换随机端口重来,最多 PORT_START_ATTEMPTS 次。
 */
async function spawnForSession(sid: string, shell: string): Promise<ShellInstance> {
  const cfg = serviceConfig;
  const dataDir = cfg?.resolveDataDir ? cfg.resolveDataDir(sid) : "";
  const title = await resolveWindowTitle(sid);
  let lastErr: unknown = null;
  for (let attempt = 1; attempt <= PORT_START_ATTEMPTS; attempt++) {
    // 显式指定端口(release 模式 GUI 程序无控制台,无法解析 stdout);随机区间避开常用端口
    const port = 18000 + Math.floor(Math.random() * 1000);
    const args = ["--browsers-path", cfg?.browsersPath || "", "--port", String(port), "--title", title];
    // 每会话独立 user-data-dir(WebView2 profile 隔离);未提供则不传(回落 WebView2 默认目录)
    if (dataDir) args.push("--user-data-dir", dataDir);
    // 推送式投递:注册**本会话**的通知地址 → 通知按 sid 归属
    const notifyUrl = getNotifyUrlForSession(sid);
    if (notifyUrl) args.push("--notify-url", notifyUrl);

    let proc: any;
    try {
      proc = spawn(shell, args, { stdio: ["ignore", "pipe", "pipe"] });
    } catch (e) {
      // spawn 同步失败(如文件被锁):换不含路径的文案,避免绝对路径经错误消息回流模型
      log.error(`spawn bt-shell failed: ${(e as Error).message}`, e as Error);
      throw new Error(tf("client.spawn_failed"));
    }
    attachStdio(proc, sid);
    const baseUrl = `http://127.0.0.1:${port}`;
    try {
      await waitForReady(baseUrl, tf("client.service_timeout"));
    } catch (e) {
      // 启动超时(常见:端口被别的进程占住 → bind 失败 → 永远不就绪):杀掉本次子进程换端口重来
      lastErr = e;
      log.error(`[session] sid=${sid} port=${port} not ready (attempt ${attempt}/${PORT_START_ATTEMPTS}); killing that pid and retrying`);
      await terminateOwnProcess(proc);
      continue;
    }
    // 就绪:把本会话的通知地址注册给该实例(推送式投递)
    await registerNotifyUrl(baseUrl, sid);
    log.info(`[session] sid=${sid} ready on port ${port} (title="${title}", dataDir=${dataDir || "(default)"}, attempt ${attempt})`);
    return { sid, proc, baseUrl, port, dataDir, title, ready: true, attach: false };
  }
  throw lastErr instanceof Error ? lastErr : new Error(tf("client.service_timeout"));
}

/** 附着到外部已运行的 shell(所有 sid 共用;不 spawn、不 kill、放弃会话隔离) */
async function attachToService(baseUrl: string, sid: string): Promise<ShellInstance> {
  log.info(`Attaching to existing bt-shell at ${baseUrl} (no spawn; session isolation disabled in attach mode)`);
  await waitForReady(baseUrl, tf("client.attach_timeout", { url: baseUrl }));
  const inst: ShellInstance = {
    sid,
    proc: null,
    baseUrl,
    port: Number(new URL(baseUrl).port || 0),
    dataDir: "",
    title: "(external)",
    ready: true,
    attach: true,
  };
  log.info(`Attached to existing bt-shell at ${baseUrl}`);
  return inst;
}

/**
 * 确保某会话的实例存在并就绪(懒启动;并发调用复用同一启动 Promise)
 *
 * **已关闭语义(用户口径)**:用户 `close` 或手动点 X 之后,该 sid 进入「已关闭」态
 * (`closedSids` 置位)—— 之后的**工具调用一律不 spawn**,只返回指引,必须由
 * `open_window`(唯一打开入口,`allowClosed=true` 且先清标记)显式打开。
 */
export async function ensureInstance(explicitSid?: string, opts?: { allowClosed?: boolean }): Promise<ShellInstance> {
  const sid = resolveSessionId(explicitSid);
  const allowClosed = opts?.allowClosed === true;

  // 已关闭态:不 spawn、不 attach,直接给指引(唯一例外是 open_window 的显式打开)
  if (!allowClosed && closedSids.has(sid)) {
    throw new Error(tf("client.browser_closed"));
  }

  // 附着模式:外部 shell 单例,所有 sid 共用(连接一次,之后直接返回)。
  // notify 语义:`/api/notify-url` 在 shell 内是**全局单值**,待发送队列也是**全局唯一**;
  // 附着模式共享同一个 shell ⇒ 队列全局,drain 由 notify URL 里带的 sid 执行。
  // 因此每个 sid 首次走 attach 分支时都必须注册自己的 notify 地址(按 baseUrl+sid 去重)。
  const attachUrl = resolveAttachBaseUrl();
  if (attachUrl) {
    if (attachInstance?.ready) {
      await ensureAttachNotify(attachInstance.baseUrl, sid);
      return attachInstance;
    }
    const existing = sessions.get(sid);
    if (existing?.attach) {
      await ensureAttachNotify(existing.baseUrl, sid);
      return existing;
    }
    const inst = await attachToService(attachUrl, sid);
    // P3-①:附着期间该 sid 被关闭 → 只解本地关联(外部进程不归插件管,绝不 kill)
    if (closedSids.has(sid)) {
      log.info(`[session] sid=${sid} was closed during attach; discarding local association`);
      throw new Error(tf("client.session_closed_during_start"));
    }
    attachInstance = inst;
    sessions.set(sid, inst);
    await ensureAttachNotify(inst.baseUrl, sid);
    await fireReady(sid);
    return inst;
  }

  const ready = sessions.get(sid);
  if (ready?.ready) return ready;
  const starting = startingSid.get(sid);
  if (starting) return starting;

  const cfg = serviceConfig;
  if (!cfg) throw new Error(tf("client.not_configured"));

  const p = (async (): Promise<ShellInstance> => {
    const shell = await resolveShellPath();
    const inst = await spawnForSession(sid, shell);
    // P3-①:启动期间该 sid 被 closeSession/closeAllSessions 关闭 → 立即自毁刚起的 shell,不注册。
    // 注意**不删** closedSids:用户已表达关闭意图,后续工具调用仍应指引 open_window(不自动重开)
    if (closedSids.has(sid)) {
      log.info(`[session] sid=${sid} was closed during startup; terminating freshly spawned pid=${inst.proc?.pid ?? "?"}`);
      await terminateOwnProcess(inst.proc);
      throw new Error(tf("client.session_closed_during_start"));
    }
    sessions.set(sid, inst);
    await fireReady(sid);
    return inst;
  })().finally(() => {
    startingSid.delete(sid);
  });
  startingSid.set(sid, p);
  return p;
}

/** 实例就绪后的统一回调(插件侧用于对该 sid 做初始 drain) */
async function fireReady(sid: string): Promise<void> {
  try {
    serviceReadyHandler?.(sid);
  } catch (e) {
    log.error("[notify] service-ready handler failed", e as Error);
  }
}

/** 把某会话的通知地址注册给该实例(/api/notify-url);直接 fetch,避免 callApi 触发 ensure 递归
 * 返回是否注册成功(失败不抛出:推送是增强路径,不能因注册失败阻断工具调用) */
async function registerNotifyUrl(baseUrl: string, sid: string): Promise<boolean> {
  const url = getNotifyUrlForSession(sid);
  if (!url) return false;
  try {
    const res = await fetch(`${baseUrl}/api/notify-url`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ url }),
    });
    if (res.ok) {
      log.info(`[notify] sid=${sid} notify-url registered: ${url}`);
      return true;
    }
    log.error(`[notify] sid=${sid} register failed: HTTP ${res.status}`);
    return false;
  } catch (e) {
    log.error("[notify] register failed", e as Error);
    return false;
  }
}

/** 附着模式:按 baseUrl+sid 去重注册 notify(附着模式共享一个 shell,队列全局) */
async function ensureAttachNotify(baseUrl: string, sid: string): Promise<void> {
  const key = `${baseUrl}|${sid}`;
  if (attachNotifyRegistered.has(key)) return;
  // 先置位:并发 ensure 不重复 POST;失败则回滚,下次再试
  attachNotifyRegistered.add(key);
  const ok = await registerNotifyUrl(baseUrl, sid);
  if (!ok) attachNotifyRegistered.delete(key);
}

/** 清掉某外部 shell 的 notify 注册记录(断开/换实例后重新连接需重新注册) */
function clearAttachNotify(baseUrl: string): void {
  const prefix = `${baseUrl}|`;
  for (const key of [...attachNotifyRegistered]) {
    if (key.startsWith(prefix)) attachNotifyRegistered.delete(key);
  }
}

/**
 * 关闭某会话的实例:优雅(/api/close)→ 等退出 → 超时才 SIGTERM(仅该实例自己的句柄/PID)→ 从注册表移除
 * 语义保证:实例不存在则**直接返回,绝不因此 spawn**;附着模式只解除本地关联,外部进程不动。
 */
export async function closeSession(explicitSid?: string): Promise<void> {
  const sid = resolveSessionId(explicitSid);
  // P3-①:先置关闭标记 —— 即使实例尚不存在(启动进行中),启动链完成后也会据此自毁,不留孤儿
  closedSids.add(sid);
  const inst = sessions.get(sid) || (attachInstance && attachInstance.sid === sid ? attachInstance : null);
  if (!inst) {
    log.info(`[session] sid=${sid} has no shell instance; nothing to close (never spawn on close)`);
    return;
  }
  sessions.delete(inst.sid);
  // 已关闭态保留 closedSids:后续工具调用不再自动重开,须 open_window 显式打开
  if (attachInstance === inst) attachInstance = null;
  if (inst.attach) {
    // 附着模式:外部 shell 进程不归插件管,**绝不 kill**,只清本地关联
    clearAttachNotify(inst.baseUrl);
    log.info(`Detached from external bt-shell at ${inst.baseUrl} (process left running)`);
    return;
  }
  try {
    await fetch(`${inst.baseUrl}/api/close`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: "{}",
    });
  } catch {
    // 进程可能已自行退出;下面的 waitForExit 会兜住
  }
  await terminateOwnProcess(inst.proc);
  log.info(`[session] sid=${sid} closed (port=${inst.port})`);
}

/** 关闭全部实例(插件 dispose / 退出);只按注册表 PID,绝不按名
 * 含**启动中**的 sid:先置 closedSids(启动链自查后自毁刚 spawn 的进程),再等启动链跑完 ——
 * 确保 dispose 返回时不留孤儿窗口。 */
export async function closeAllSessions(): Promise<void> {
  const starting = [...startingSid.values()];
  const sids = new Set<string>([...sessions.keys(), ...startingSid.keys()]);
  for (const sid of sids) {
    await closeSession(sid);
  }
  // 启动链可能仍在 waitForReady:等它自查 closedSids 并终止自己 spawn 的进程(失败也不阻塞 dispose)
  await Promise.allSettled(starting);
}

/** 该会话是否正处于启动中(close 需要能关掉「还没就绪」的实例) */
export function isStarting(sid?: string): boolean {
  return startingSid.has(resolveSessionId(sid));
}

/** 清「已关闭」标记(会话删除时用,避免集合无限增长;open_window 亦会清) */
export function clearClosedState(sid: string): void {
  closedSids.delete(sid);
}

/** 打开浏览器窗口(未启动则启动该会话实例;已启动幂等)
 * 手动关窗后注册表里的实例可能仍是 ready(进程已死/窗口已关)→ 先做一次存活探测,
 * 探测失败就摘除死实例再走 ensureInstance,避免「以为已打开」而空转。 */
export async function openWindow(sid?: string): Promise<void> {
  // 唯一打开入口:显式清「已关闭」标记后再启动
  const s = resolveSessionId(sid);
  closedSids.delete(s);
  const inst = sessions.get(s);
  if (inst?.ready) {
    const alive = await probeOnce(inst.baseUrl, 800);
    if (!alive) {
      log.info(`[session] sid=${s} open_window: probe failed (port=${inst.port}); discarding dead instance before reopening`);
      await markInstanceDead(s, inst);
      if (sessions.get(s) === inst) sessions.delete(s);
      if (attachInstance === inst) attachInstance = null;
    } else {
      // 探测通过:实例仍存活,保持幂等直接返回
      return;
    }
  }
  await ensureInstance(s, { allowClosed: true });
}

/**
 * 纯查询:探测某会话的 shell 是否在运行,**绝不 spawn**。
 * sid 未给 → 先探当前会话,再探任一已就绪实例;都没有则返回 null。
 * 用于 get_browser_status 这类只读命令,避免查询也弹出浏览器窗口。
 */
export async function probeStatus(timeoutMs = 1500, sid?: string): Promise<any | null> {
  const targets: string[] = [];
  if (sid) {
    const key = resolveSessionId(sid);
    const inst = sessions.get(key);
    if (inst?.ready) targets.push(inst.baseUrl);
    // 附着模式:实例可能只挂在 attachInstance 上(共享单例,放弃按 sid 隔离)→ 同样纳入探测
    if (attachInstance?.ready && !targets.includes(attachInstance.baseUrl)) targets.push(attachInstance.baseUrl);
  } else {
    const cur = sessions.get(resolveSessionId());
    if (cur?.ready) targets.push(cur.baseUrl);
    for (const inst of sessions.values()) {
      if (inst.ready && !targets.includes(inst.baseUrl)) targets.push(inst.baseUrl);
    }
    if (attachInstance?.ready && !targets.includes(attachInstance.baseUrl)) targets.push(attachInstance.baseUrl);
  }
  for (const baseUrl of targets) {
    const r = await probeOnce(baseUrl, timeoutMs);
    if (r) return r;
  }
  return null;
}

/** 对单个端点做一次短超时探测 */
async function probeOnce(baseUrl: string, timeoutMs: number): Promise<any | null> {
  const ac = new AbortController();
  const timer = setTimeout(() => ac.abort(), timeoutMs);
  try {
    const res = await fetch(`${baseUrl}/api/status`, {
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

/**
 * 判断是否为**连接层**错误(shell 进程已死/端口没人听 → fetch 直接抛)。
 * 只认这类;HTTP 4xx/5xx 不算(它们不会抛错,而是 res.ok=false → 走 json.success 分支)。
 */
function isConnectionError(e: unknown): boolean {
  if (!(e instanceof Error)) return false;
  // undici/node 的连接失败:message = "fetch failed",真实原因在 cause.code;部分场景直接带码
  const code = String((e as any)?.cause?.code || (e as any)?.code || "");
  if (/ECONNREFUSED|ECONNRESET|EPIPE|ENOTFOUND|EAI_AGAIN|EHOSTUNREACH|ENETUNREACH|UND_ERR|ETIMEDOUT/.test(code)) {
    return true;
  }
  // 退化:message 里出现连接类字样(不同 node/undici 版本的文案差异)
  return /fetch failed|socket hang up|other side closed|connect ECONN|network error/i.test(e.message);
}

/** 把一个已死实例从注册表摘除并回收残留句柄(只按该实例自己的句柄/PID,绝不按名) */
async function markInstanceDead(sid: string, inst: ShellInstance): Promise<void> {
  // 只在注册项仍指向该实例时删除(避免误删之后由 open_window 重新注册的新实例)
  if (sessions.get(sid) === inst) sessions.delete(sid);
  if (attachInstance === inst) attachInstance = null;
  // 外部 shell 已不可达:清 notify 注册记录,重新附着时才会重新注册
  if (inst.attach) clearAttachNotify(inst.baseUrl);
  if (inst.proc) await terminateOwnProcess(inst.proc);
}

/** 发一次 POST 并解析 {success,data};连接层错误直接抛(fetch 的 TypeError 等) */
async function postJson(baseUrl: string, path: string, payload: unknown): Promise<any> {
  const res = await fetch(`${baseUrl}${path}`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(payload ?? {}),
  });
  const json = await res.json();
  if (!json.success) throw new Error(json.error || tf("client.service_error"));
  return json.data;
}

/**
 * 统一 API 调用:按 sid 路由到该会话的实例(懒启动)。
 * sid 来源:显式 sessionId 参数 > body._sessionId > 钩子缓存兜底 > "default" 桶。
 *
 * P3-②:连接层错误(手关窗口/进程被杀)→ 标记实例死亡 + 置「已关闭」态 → 抛 `client.browser_closed`
 * 指引,**绝不自动重开**(唯一打开入口是 `open_window`)。**例外**:
 * - 附着模式:外部进程不归插件管 → 只报 `client.attach_gone`,不动任何进程;
 * - get_browser_status(走 probeStatus/probeOnce,不经本函数)保持纯查询,不 spawn;
 * - HTTP 4xx/5xx 不算连接错误,原样抛服务端错误。
 */
async function callApi(path: string, body?: any, sessionId?: string): Promise<any> {
  const sid = resolveSessionId(sessionId || (body && body._sessionId));
  const inst = await ensureInstance(sid);
  const payload = { ...(body || {}), _sessionId: sid };
  try {
    return await postJson(inst.baseUrl, path, payload);
  } catch (e) {
    if (!isConnectionError(e)) throw e;
    if (inst.attach) {
      // 附着模式:不拥有该进程,只报明确错误(不 respawn)
      throw new Error(tf("client.attach_gone", { url: inst.baseUrl }));
    }
    log.error(
      `[session] sid=${sid} instance dead (port=${inst.port}, pid=${inst.proc?.pid ?? "?"}): ${(e as Error).message}; marking closed`,
    );
    await markInstanceDead(sid, inst);
    // 用户口径:手动关窗**绝不自动重开** → 置位「已关闭」态,指引显式 open_window(不做 respawn 重试)
    closedSids.add(sid);
    throw new Error(tf("client.browser_closed"));
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
  tabs: cmd("tabs"),
  annotateToggle: cmd("annotate/toggle"),
  annotateRecords: cmd("annotate/records"),
  annotateSend: cmd("annotate/send"),
  annotateConsumeSent: cmd("annotate/consume-sent"),
};
