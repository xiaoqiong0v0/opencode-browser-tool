// bt-shell 客户端:每会话独立 shell 实例(1.9.0 会话隔离架构)
//
// 架构:实例注册表 Map<sid, ShellInstance> —— 同一 opencode 进程内的不同会话各持有
// 自己的 bt-shell 进程(独立端口 + 独立 --user-data-dir + 独立窗口标题),互不共享
// 窗口/标签/UA/批注等一切 shell 状态;进程回收只按注册表里的 PID,**绝不按名杀进程**。
// 附着模式(BT_SHELL_URL/BT_SHELL_PORT)例外:连接外部单例,放弃会话隔离(见 docs/implementation.md)。
import { spawn } from "child_process";
import { randomBytes } from "crypto";
import { resolve, dirname } from "path";
import { existsSync } from "fs";
import { fileURLToPath } from "url";
import { log } from "./logger.js";
import { getNotifyUrlForSession } from "./notify-server.js";
import { tf } from "./i18n/index.js";

import { cachedBinaryPath, describeBinaryStatus, ensureBinary } from "./binary.js";
import { sanitizeForModel } from "./util/sanitize.js";

const __dirname = dirname(fileURLToPath(import.meta.url));

/** 就绪轮询超时(ms) */
const READY_TIMEOUT_MS = 20000;
/** 就绪轮询间隔(ms) */
const READY_POLL_INTERVAL_MS = 300;
/** 端口撞车/启动失败时的重试次数(每次换一个随机端口) */
const PORT_START_ATTEMPTS = 5;
/** 会话标题解析上限(ms):超时就用 sid 后缀,绝不因标题卡住启动 */
const TITLE_RESOLVE_TIMEOUT_MS = 250;
/** HTTP 401 Unauthorized:shell 明确判定令牌不匹配(rust/src/http.rs 的 auth_error) */
const HTTP_UNAUTHORIZED = 401;
/** HTTP 403 Forbidden:shell 明确拒绝该请求(带 Origin 的跨源请求) */
const HTTP_FORBIDDEN = 403;

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

/** 附着模式令牌:由 BT_SHELL_TOKEN 提供(外部 shell 须用同一值启动);缺失直接抛明确错误 */
function requireAttachToken(): string {
  const token = process.env.BT_SHELL_TOKEN?.trim();
  if (!token) throw new Error(tf("client.attach_need_token"));
  return token;
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
  /** HTTP API 鉴权令牌(实例创建期确定,随实例不可变;所有 /api/* 请求带 x-bt-token) */
  token: string;
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
/**
 * 同一 key 的注册**进行中** Promise:并发 ensure 共享同一次 POST(第二个调用 await 第一个结果,
 * 而不是直接返回)。失败时只删这里,使下一次调用能重试;成功才写入 attachNotifyRegistered。
 */
const attachNotifyInFlight = new Map<string, Promise<boolean>>();

/** notify-url 注册失败后的重试延迟(ms) */
const NOTIFY_RETRY_DELAY_MS = 300;

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

/**
 * 轮询等待服务就绪(READY_TIMEOUT_MS 超时);超时抛出给定提示。
 *
 * `authHint`(仅附着模式传 true):401/403 是**确定性失败** —— shell 已经起来并明确拒绝鉴权,
 * 令牌不一致在它启动那一刻就定死了,重试到超时也永远不会变成 200,继续轮询只会白等
 * READY_TIMEOUT_MS(20s)再报一句无关的超时。因此一旦拿到 401/403 立即抛鉴权错误。
 * spawn 路径不传(authHint=false):令牌由插件自己随机生成并原样透传 --token,
 * 出现 401 属异常(端口上坐着别的进程等),保持原有"继续重试→超时"路径。
 */
async function waitForReady(
  baseUrl: string,
  token: string,
  timeoutMessage: string,
  authHint = false,
): Promise<void> {
  const deadline = Date.now() + READY_TIMEOUT_MS;
  while (Date.now() < deadline) {
    // 就绪语义:只有 ok 算起来;transport(端口还没监听)、server/badjson 都属于
    // 「还没可用」继续等;auth 在附着模式是确定性失败(令牌写错),立即报错不再空等。
    const out = await httpCall(baseUrl, token, "/api/status", {});
    if (out.kind === "ok") return;
    if (out.kind === "auth" && authHint) throw new Error(tf("client.attach_bad_token"));
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
  // 本实例的鉴权令牌:16 字节 → 32 hex;端口重试沿用同一令牌(新的子进程仍是同一实例语义)
  const token = randomBytes(16).toString("hex");
  let lastErr: unknown = null;
  for (let attempt = 1; attempt <= PORT_START_ATTEMPTS; attempt++) {
    // 显式指定端口(release 模式 GUI 程序无控制台,无法解析 stdout);随机区间避开常用端口
    const port = 18000 + Math.floor(Math.random() * 1000);
    const args = [
      "--browsers-path",
      cfg?.browsersPath || "",
      "--port",
      String(port),
      "--title",
      title,
      "--token",
      token,
    ];
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
      // authHint 默认 false:spawn 的令牌是插件本会话随机生成并随 --token 传下去的,
      // 不该出现 401;真出现(如端口被别的进程占住)属于"没起来"一类,走换端口重试而不是鉴权报错
      await waitForReady(baseUrl, token, tf("client.service_timeout"));
    } catch (e) {
      // 启动超时(常见:端口被别的进程占住 → bind 失败 → 永远不就绪):杀掉本次子进程换端口重来
      lastErr = e;
      log.error(`[session] sid=${sid} port=${port} not ready (attempt ${attempt}/${PORT_START_ATTEMPTS}); killing that pid and retrying`);
      await terminateOwnProcess(proc);
      continue;
    }
    // 就绪:把本会话的通知地址注册给该实例(推送式投递;失败重试一次,不影响工具调用成功)
    const notifyOk = await registerNotifyUrl(baseUrl, sid, token);
    if (!notifyOk) {
      await new Promise((r) => setTimeout(r, NOTIFY_RETRY_DELAY_MS));
      const retried = await registerNotifyUrl(baseUrl, sid, token);
      if (!retried) log.error(`[notify] sid=${sid} notify-url registration failed after retry (delivery may be degraded)`);
    }
    log.info(`[session] sid=${sid} ready on port ${port} (title="${title}", dataDir=${dataDir || "(default)"}, attempt ${attempt})`);
    return { sid, proc, baseUrl, port, token, dataDir, title, ready: true, attach: false };
  }
  throw lastErr instanceof Error ? lastErr : new Error(tf("client.service_timeout"));
}

/** 附着到外部已运行的 shell(所有 sid 共用;不 spawn、不 kill、放弃会话隔离) */
async function attachToService(baseUrl: string, sid: string): Promise<ShellInstance> {
  // 令牌必须先有:缺失时直接报错,不去连一个注定 401 的端点
  const token = requireAttachToken();
  log.info(`Attaching to existing bt-shell at ${baseUrl} (no spawn; session isolation disabled in attach mode)`);
  // authHint=true:令牌来自 BT_SHELL_TOKEN(与外部 shell 的启动令牌是两处配置),
  // 401/403 说明两边不一致 → 立即报鉴权失败,不等 20s 超时
  await waitForReady(baseUrl, token, tf("client.attach_timeout", { url: baseUrl }), true);
  const inst: ShellInstance = {
    sid,
    proc: null,
    baseUrl,
    port: Number(new URL(baseUrl).port || 0),
    token,
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
      await ensureAttachNotify(attachInstance, sid);
      return attachInstance;
    }
    const existing = sessions.get(sid);
    if (existing?.attach) {
      await ensureAttachNotify(existing, sid);
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
    await ensureAttachNotify(inst, sid);
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
    // P0 竞态收尾:本轮是 open_window 的显式打开 → 成功后清「已关闭」标记。
    // 期间若有并发 callApi 探活失败把 sid 重新置为 closed,这里也要清掉,
    // 否则会出现「窗口明明开着,后续工具却报已关闭」。
    if (allowClosed) closedSids.delete(sid);
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
async function registerNotifyUrl(baseUrl: string, sid: string, token: string): Promise<boolean> {
  const url = getNotifyUrlForSession(sid);
  if (!url) return false;
  // 增强路径:只有 ok 算成功,其余(transport/auth/server/badjson)都降级为 false,
  // 由调用方决定是否重试;绝不因注册失败影响工具调用或实例存活判定。
  const out = await httpCall(baseUrl, token, "/api/notify-url", { url });
  if (out.kind === "ok") {
    log.info(`[notify] sid=${sid} notify-url registered: ${url}`);
    return true;
  }
  const detail =
    out.kind === "auth" ? `HTTP ${out.status}` : out.kind === "server" ? `server: ${out.error}` : out.kind;
  log.error(`[notify] sid=${sid} register failed: ${detail}`);
  return false;
}

/** 附着模式:按 baseUrl+sid 去重注册 notify(附着模式共享一个 shell,队列全局)
 * 并发语义:同 key 的并发调用共享同一次 POST(await 同一个 Promise),失败只清 in-flight,
 * 使下一次调用自动重试;成功才写入「已注册」集合。 */
async function ensureAttachNotify(inst: ShellInstance, sid: string): Promise<void> {
  const key = `${inst.baseUrl}|${sid}`;
  if (attachNotifyRegistered.has(key)) return;
  const inflight = attachNotifyInFlight.get(key);
  if (inflight) {
    await inflight;
    return;
  }
  const p = registerNotifyUrl(inst.baseUrl, sid, inst.token);
  attachNotifyInFlight.set(key, p);
  let ok = false;
  try {
    ok = await p;
  } finally {
    // settle 后清理,避免 Promise 常驻导致内存泄漏
    if (attachNotifyInFlight.get(key) === p) attachNotifyInFlight.delete(key);
  }
  if (ok) attachNotifyRegistered.add(key);
}

/** 清掉某外部 shell 的 notify 注册记录(断开/换实例后重新连接需重新注册) */
function clearAttachNotify(baseUrl: string): void {
  const prefix = `${baseUrl}|`;
  for (const key of [...attachNotifyRegistered]) {
    if (key.startsWith(prefix)) attachNotifyRegistered.delete(key);
  }
  for (const key of [...attachNotifyInFlight.keys()]) {
    if (key.startsWith(prefix)) attachNotifyInFlight.delete(key);
  }
}

/** 清掉某 sid 的 attach notify 记录(会话删除时调用;键格式为 `baseUrl|sid`) */
export function clearAttachNotifyForSid(sid: string): void {
  if (!sid) return;
  const suffix = `|${sid}`;
  for (const key of [...attachNotifyRegistered]) {
    if (key.endsWith(suffix)) attachNotifyRegistered.delete(key);
  }
  for (const key of [...attachNotifyInFlight.keys()]) {
    if (key.endsWith(suffix)) attachNotifyInFlight.delete(key);
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
  // 关闭路径 best-effort:任何结果都忽略(进程可能已自行退出,transport 也正常),
  // 存活收尾交给下面的 terminateOwnProcess/waitForExit。
  await httpCall(inst.baseUrl, inst.token, "/api/close", {});
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
    const alive = await probeOnce(inst.baseUrl, 800, inst.token);
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
  // 启动期间并发的 callApi 可能因连接层错误再次置 closedSids(其探活的是旧实例);
  // 本窗口已确认打开 → 成功路径再清一次,避免「窗口开着却报已关闭」。
  closedSids.delete(s);
}

/**
 * 纯查询:探测某会话的 shell 是否在运行,**绝不 spawn**。
 * sid 未给 → 先探当前会话,再探任一已就绪实例;都没有则返回 null。
 * 用于 get_browser_status 这类只读命令,避免查询也弹出浏览器窗口。
 */
export async function probeStatus(timeoutMs = 1500, sid?: string): Promise<any | null> {
  // 按实例探测(令牌随实例走:spawn 实例用自己生成的令牌,attach 实例用 BT_SHELL_TOKEN)
  const targets: ShellInstance[] = [];
  const add = (inst: ShellInstance | null | undefined): void => {
    if (inst?.ready && !targets.some((t) => t.baseUrl === inst.baseUrl)) targets.push(inst);
  };
  if (sid) {
    add(sessions.get(resolveSessionId(sid)));
    // 附着模式:实例可能只挂在 attachInstance 上(共享单例,放弃按 sid 隔离)→ 同样纳入探测
    add(attachInstance);
  } else {
    add(sessions.get(resolveSessionId()));
    for (const inst of sessions.values()) add(inst);
    add(attachInstance);
  }
  for (const inst of targets) {
    const r = await probeOnce(inst.baseUrl, timeoutMs, inst.token);
    if (r) return r;
  }
  return null;
}

/** 对单个端点做一次短超时探测:只有 ok 有值;其余(auth/server/badjson/transport)一律 null */
async function probeOnce(baseUrl: string, timeoutMs: number, token: string): Promise<any | null> {
  const out = await httpCall(baseUrl, token, "/api/status", {}, { timeoutMs });
  return out.kind === "ok" ? out.data : null;
}

/**
 * 一次 HTTP 调用的**分类结果**。
 *
 * 本原语只负责「发请求 + 归类」,**不决定抛还是吞** —— 抛/吞/重试策略由各调用方
 * 按自身语义映射(见 `httpCall` 注释的对照表),避免在每个 fetch 点各写补丁。
 */
type HttpOutcome =
  /** 传输成功且服务端 success=true */
  | { kind: "ok"; data: any }
  /** 服务端明确 success=false(业务错误;实例仍存活) */
  | { kind: "server"; error: string }
  /** 401/403:令牌不匹配或被拒绝(确定性鉴权失败,不是连接故障) */
  | { kind: "auth"; status: number }
  /** fetch 本身抛错或请求超时(端口没人听/进程已死/超时),已标记为 ConnectionError */
  | { kind: "transport"; error: ConnectionError }
  /** 拿到了 HTTP 响应但响应体不是 JSON(协议异常,不等于实例死亡) */
  | { kind: "badjson" };

/**
 * 单一 HTTP 原语:POST JSON 到 bt-shell 本地 API 并归类结果。**从不抛错**,也不决定
 * 调用方语义(唯一出口是 `HttpOutcome`)。
 *
 * 统一请求头(`Content-Type` + `x-bt-token`)与统一超时(AbortController;`timeoutMs`
 * 省略则不限,与原实现一致)。归类顺序:fetch 抛错/超时 → transport;401/403 → auth;
 * 响应体非 JSON → badjson;其余看 `success` 字段。
 *
 * 策略分工(调用方各自映射,不在本函数里做):
 * - `postJson`(工具调用):ok→data、server→抛错、auth→抛鉴权错、transport→抛
 *   `ConnectionError`(供 callApi 判定实例死亡)、badjson→抛解析错(不判死);
 * - `probeOnce`(探活):只有 ok 有值,其余 null;
 * - `registerNotifyUrl`(增强路径):只有 ok 返回 true,其余 false;
 * - `waitForReady`(就绪轮询):ok 结束,transport/server/badjson 继续等,attach 的 auth 立即报错;
 * - `closeSession`(关闭):忽略任何结果。
 */
async function httpCall(
  baseUrl: string,
  token: string,
  path: string,
  payload: unknown,
  opts?: { timeoutMs?: number },
): Promise<HttpOutcome> {
  const timeoutMs = opts?.timeoutMs && opts.timeoutMs > 0 ? opts.timeoutMs : 0;
  const ac = timeoutMs > 0 ? new AbortController() : null;
  const timer = ac ? setTimeout(() => ac.abort(), timeoutMs) : null;
  try {
    let res: Response;
    try {
      res = await fetch(`${baseUrl}${path}`, {
        method: "POST",
        headers: { "Content-Type": "application/json", "x-bt-token": token },
        body: JSON.stringify(payload ?? {}),
        signal: ac?.signal,
      });
    } catch (e) {
      // 超时与连接失败同归 transport:由抛错点显式标记类型,调用方无需识别运行时文案;
      // 超时在 cause 里留下标记,便于日志区分「端口没监听」与「服务没响应」。
      const timedOut = ac?.signal.aborted === true;
      return {
        kind: "transport",
        error: timedOut
          ? new ConnectionError(`request timed out after ${timeoutMs}ms`, { timeout: true, cause: e })
          : new ConnectionError((e as Error)?.message || "connection failed", e),
      };
    }
    if (res.status === HTTP_UNAUTHORIZED || res.status === HTTP_FORBIDDEN) {
      return { kind: "auth", status: res.status };
    }
    let json: any;
    try {
      json = await res.json();
    } catch {
      return { kind: "badjson" };
    }
    if (json?.success) return { kind: "ok", data: json.data };
    return { kind: "server", error: typeof json?.error === "string" ? json.error : "" };
  } finally {
    if (timer) clearTimeout(timer);
  }
}

/**
 * 传输层连接错误:`fetch` **本身**抛错(端口没人听 / shell 进程已死)或请求超时时,
 * 由 `httpCall` 标记并用 `HttpOutcome.transport` 交给调用方。
 *
 * 为什么要显式类型:同一份代码会跑在不同运行时上 —— opencode 默认跑在 **Bun**,Bun 的 fetch
 * 失败是 `TypeError: Unable to connect. Is the computer able to access the url?`(无 `cause.code`);
 * Node/undici 是 `TypeError: fetch failed`,真实原因在 `cause.code=ECONNREFUSED`。
 * 靠猜文案/码跨运行时必漏(1.10.1 真机缺陷),因此传输层失败**由抛错点标记类型**,判断方只认类型。
 */
export class ConnectionError extends Error {
  /** 原始 fetch 异常(保留运行时细节,便于日志取证) */
  override cause?: unknown;

  /**
   * @param message 人类可读的连接失败信息(通常取原始异常 message)
   * @param cause 原始 fetch 异常
   */
  constructor(message: string, cause?: unknown) {
    super(message);
    this.name = "ConnectionError";
    if (cause !== undefined) this.cause = cause;
  }
}

/**
 * 判断是否为**连接层**错误(shell 进程已死/端口没人听 → fetch 直接抛)。
 * 只认这类;HTTP 4xx/5xx 不算(它们是正常响应,由 `httpCall` 归入 auth/server 分支)。
 *
 * 优先级:显式 `ConnectionError`(运行时无关) → Node/undici 的码/文案(兼容旧路径) →
 * Bun 等运行时的文案兜底(`Unable to connect` / `ConnectionRefused` / `Failed to connect`)。
 * 导出仅供自验/单测直接构造运行时文案验证。
 */
export function isConnectionError(e: unknown): boolean {
  // 首选:传输层抛错点已显式标记类型(不依赖任何运行时文案)
  if (e instanceof ConnectionError) return true;
  if (!(e instanceof Error)) return false;
  // 兼容 Node/undici:message = "fetch failed",真实原因在 cause.code;部分场景直接带码。
  // `ConnectionRefused` 是 Bun 的码名(大小写不固定,故 i 忽略大小写)。
  const code = String((e as any)?.cause?.code || (e as any)?.code || "");
  if (/ECONNREFUSED|ECONNRESET|EPIPE|ENOTFOUND|EAI_AGAIN|EHOSTUNREACH|ENETUNREACH|UND_ERR|ETIMEDOUT|ConnectionRefused/i.test(code)) {
    return true;
  }
  // 文案兜底(多运行时):Bun 无 cause.code 只有 "Unable to connect...";Node/undici 各有自己的措辞。
  // 只在这里做兜底,正常路径以 ConnectionError 类型为准。
  return /fetch failed|socket hang up|other side closed|connect ECONN|network error|Unable to connect|ConnectionRefused|Failed to connect/i.test(
    e.message,
  );
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

/**
 * 工具调用链路的 POST:把 `httpCall` 的分类**映射为对外语义**(这里是唯一决定抛/吞的地方)。
 * 映射规则见 `httpCall` 注释;其中 auth 在自建实例上属异常(令牌由插件自己生成),
 * 报鉴权错误而**不**报连接错误。
 */
async function postJson(inst: ShellInstance, path: string, payload: unknown): Promise<any> {
  const out = await httpCall(inst.baseUrl, inst.token, path, payload);
  switch (out.kind) {
    case "ok":
      return out.data;
    case "server":
      // 服务端错误文本可能夹带底层路径(如 WebView2/fs 细节):回模型前统一脱敏
      throw new Error(sanitizeForModel(out.error || tf("client.service_error")));
    case "auth":
      throw new Error(tf(inst.attach ? "client.attach_bad_token" : "client.auth_rejected"));
    case "transport":
      throw out.error;
    case "badjson":
      throw new Error(tf("client.bad_response"));
  }
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
    return await postJson(inst, path, payload);
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
