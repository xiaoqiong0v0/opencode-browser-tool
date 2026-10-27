import { readFileSync, writeFileSync, readdirSync, unlinkSync, mkdirSync, rmSync, existsSync, statSync } from "fs";
import { resolve } from "path";
import stringArgv from "string-argv";
import { parseArgs } from "node:util";
import {
  loadConfig, getConfig, getBrowsersDir, getCacheDir, getProfilesDir,
  getExportsDir, getTmpDir, getActiveProfile, setActiveProfile, getSessionDataDir, getSessionTmpDir,
  getSessionsDir, resolveProfilePath, isInsideDir,
} from "./config/index.js";
import { registerLocale, tf } from "./i18n/index.js";
import en from "./i18n/en.js";
import zh from "./i18n/zh.js";
import {
  configureService, setAppDirectory, getAppDirectory, setServiceReadyHandler, setSessionIdResolver, setTitleResolver,
  resolveSessionId, openWindow, isRunning, isStarting, closeSession, closeAllSessions, clearClosedState, service, probeStatus,
  clearAttachNotifyForSid,
} from "./client.js";
import { startNotifyServer } from "./notify-server.js";
import { startDownload, getBinaryStatus, describeBinaryStatus } from "./binary.js";
import { sanitizeForModel } from "./util/sanitize.js";
import {
  EVAL_ITEM_MAX_BYTES, EVAL_TOTAL_MAX_BYTES, classifyEvalArgs, runEvalItems,
  type EvalFileRead, type EvalItem,
} from "./util/eval-items.js";
import {
  INJECT_RULE_MAX_BYTES, buildInjectItems, formatInjectAdd, formatInjectList, formatInjectTest,
  injectAddIssue, injectRuleLimitIssue, normalizeInjectAction,
  type InjectItem, type InjectRulesData,
} from "./util/inject-rules.js";
import {
  formatConsoleItems, formatNetworkItems, renderLogText,
  type ConsoleLogItem, type NetworkLogItem, type LogStreamData,
} from "./util/log-format.js";
import {
  TYPE_TEXT_MAX_BYTES, resolveTypeText, typeTextIssue, formatTypeOutput,
} from "./util/type-text.js";
import { tool, type Plugin } from "@opencode-ai/plugin";
import { log } from "./logger.js";
import { createSessionIDCache, extractSessionIDFromEvent } from "./session-id.js";
import { createSessionTree } from "./session-tree.js";

/** 取 i18n 文案(tf 的快捷别名;vars 可选,替换 {name} 占位符) */
const _t = (k: string, vars?: Record<string, string | number>) => tf(k, vars);

/** 当前会话 id(工具调用的显式 _sessionId 优先,否则用钩子缓存兜底) */
function sidOf(a: any): string {
  return resolveSessionId(typeof a?._sessionId === "string" ? a._sessionId : "");
}

/** 取 raw 会话最终归属的浏览器 key(私有=自身;否则沿 parentID 归并到根;失败回退 raw,不抛) */
async function browserKeyOf(raw: string): Promise<string> {
  try {
    return await sessionTree.browserKey(raw);
  } catch {
    return raw || "default";
  }
}

/** 构造只带会话 id 的请求体(用于无参但需按会话路由的命令,如 list_tabs / reload) */
function sidArgs(a: any): { _sessionId: string } {
  return { _sessionId: sidOf(a) };
}

/** 降级说明:服务端在无可信通道、退回 untrusted DOM/JS 时返回 degraded,统一在此追加标注 */
function degradedNote(r: any): string {
  return r && r.degraded ? _t("cli.note.degraded", { reason: r.degradedReason || "untrusted fallback" }) : "";
}

/** 就绪说明:导航/前进后退未在超时内就绪时如实标注(不假装成功) */
function readyNote(r: any): string {
  if (!r || r.ready !== false) return "";
  const rs = r.readyState ? `, readyState=${r.readyState}` : "";
  return _t("cli.note.not_ready", { ms: r.waitedMs ?? "?", rs });
}

/** 滚动稳定说明:未在超时内稳定时如实标注 */
function stableNote(r: any): string {
  if (!r || r.stable !== false) return "";
  return _t("cli.note.scroll_unstable", { ms: r.waitedMs ?? "?" });
}

/** scroll_to_element 到达说明:未进入视口时如实标注(含原因) */
function reachedNote(r: any): string {
  if (!r || r.reached !== false) return "";
  const why = r.reason ? `: ${r.reason}` : "";
  return _t("cli.note.reached", {
    where: r.inViewport ? "in viewport" : "not in viewport",
    ms: r.waitedMs ?? "?",
    why,
  });
}

/** CLI 命令参数定义:flag=参数名,type=类型(string/boolean),multiple=可重复传入(值恒为数组) */
type CmdArg = { flag: string; type?: "string" | "boolean"; multiple?: boolean };
/** CLI 命令定义:usage=参数用法,descKey=命令描述 i18n 键(cli.cmd.*),args=可解析的 flag 表,
 *  run=执行(第二个入参是该命令的位置参数,只有 evaluate 使用,其余命令忽略) */
type CmdDef = {
  usage: string;
  descKey: string;
  args?: CmdArg[];
  run: (a: any, positionals: string[]) => Promise<any>;
};

/** 命令表:bt_cli 按 command 分发;命令描述全部走 i18n 键,帮助按 toolLang 本地化 */

/** `--timeout` 合法区间(ms):与 Rust `service.rs` 的 `TIMEOUT_MIN_MS`/`TIMEOUT_MAX_MS` 同口径 */
const TIMEOUT_MIN_MS = 100;
const TIMEOUT_MAX_MS = 60000;

/** 解析并规范化模型传入的 `--timeout`(毫秒,与 `wait_for_selector` 既有单位一致)
 * - 未传/空:返回空串,各命令走服务端默认(1.9.0 批次4 起 eval/CDP/截图/wait_for_selector 均 15s)
 * - 非正整数:抛错 —— 快速明确拒绝,不带着坏值进浏览器
 * - 超出 [100, 60000]:钳制并把 `a.timeout` 改写为钳后值(实际生效值),返回如实标注文案
 * 服务端 Rust 侧同口径二次钳制兜底(直连 HTTP 也安全);响应体的 timeoutClamped 与这里一致
 */
function timeoutNote(a: any): string {
  const raw = a?.timeout;
  if (raw === undefined || raw === null || raw === "") return "";
  const n = typeof raw === "number" ? raw : Number(String(raw).trim());
  if (!Number.isInteger(n) || n <= 0) {
    throw new Error(_t("cli.err.timeout_invalid", { v: String(raw) }));
  }
  const c = Math.min(Math.max(n, TIMEOUT_MIN_MS), TIMEOUT_MAX_MS);
  a.timeout = c;
  return c === n ? "" : _t("cli.note.timeout_clamped", { ms: c });
}

/** evaluate 请求体：脚本 + 会话 id + 已钳制的 timeout（CLI 专用 flag 不下发） */
function evalPayload(a: any, script: string): Record<string, any> {
  const p: Record<string, any> = { script, _sessionId: a._sessionId };
  if (a.timeout !== undefined) p.timeout = a.timeout;
  return p;
}

/** `@file:` 允许目录：全局 tmp + 指定会话 tmp（sid 形态非法时回落 default 桶，防止路径越界） */
function allowedFileDirs(browserSid: string): { globalTmpDir: string; dirs: string[] } {
  const fileSid = SESSION_SID_RE.test(browserSid) ? browserSid : "default";
  const globalTmpDir = getTmpDir();
  return { globalTmpDir, dirs: [globalTmpDir, getSessionTmpDir(fileSid)] };
}

/** 文件项读取的错误文案键集合（evaluate / inject 共用读取与 containment 逻辑，仅文案前缀不同） */
type FileReadKeys = { outside: string; tooLarge: string; totalTooLarge: string; notAFile: string; readFailed: string };

/** `evaluate` 文件项错误文案键 */
const EVAL_FILE_KEYS: FileReadKeys = {
  outside: "cli.err.eval_file_outside",
  tooLarge: "cli.err.eval_file_too_large",
  totalTooLarge: "cli.err.eval_total_too_large",
  notAFile: "cli.err.eval_not_a_file",
  readFailed: "cli.read_file_failed",
};

/** `inject` 文件项错误文案键（读取/containment 逻辑复用 evaluate，上限换为单规则口径） */
const INJECT_FILE_KEYS: FileReadKeys = {
  outside: "cli.err.inject_file_outside",
  tooLarge: "cli.err.inject_file_too_large",
  totalTooLarge: "cli.err.inject_rule_too_large",
  notAFile: "cli.err.eval_not_a_file",
  readFailed: "cli.err.inject_read_failed",
};

/** `type --text @file:` 错误文案键（读取/containment 逻辑复用 evaluate，文案前缀换成 type） */
const TYPE_FILE_KEYS: FileReadKeys = {
  outside: "cli.err.type_file_outside",
  tooLarge: "cli.err.type_file_too_large",
  totalTooLarge: "cli.err.type_file_total_too_large",
  notAFile: "cli.err.eval_not_a_file",
  readFailed: "cli.err.type_read_failed",
};

/**
 * 读取一个文件项（evaluate 的 `@file:<path>` / inject 的 `--file <path>`）。
 *
 * 安全边界：先做 containment 校验（工作区目录 / 全局 tmp / 会话 tmp 之内），不通过**直接拒绝且不读取**；
 * 相对路径的基准是**工作区目录**（不是插件 cwd）。再校验单文件 ≤256KB、合计 ≤`totalMaxBytes`
 * （默认 1MB；inject 传单规则上限）。
 *
 * @param pathArg 模型传入的路径（原样用于文案回显）
 * @param ctx workspaceDir=工作区目录（可能为空）、allowedDirs=允许目录、usedBytes=已累计字节、
 *   totalMaxBytes=合计上限（缺省 `EVAL_TOTAL_MAX_BYTES`）
 * @param keys 错误文案键集合（缺省 evaluate 的一套）
 * @returns 读到的脚本文本与字节数，或错误文案
 */
function readEvalFile(
  pathArg: string,
  ctx: { workspaceDir: string; allowedDirs: string[]; usedBytes: number; totalMaxBytes?: number },
  keys: FileReadKeys = EVAL_FILE_KEYS,
): EvalFileRead {
  const abs = ctx.workspaceDir ? resolve(ctx.workspaceDir, pathArg) : resolve(pathArg);
  if (!ctx.allowedDirs.some((d) => isInsideDir(abs, d))) {
    return { error: _t(keys.outside, { path: pathArg }) };
  }
  const totalMax = ctx.totalMaxBytes && ctx.totalMaxBytes > 0 ? ctx.totalMaxBytes : EVAL_TOTAL_MAX_BYTES;
  let size = 0;
  try {
    const st = statSync(abs);
    if (!st.isFile()) {
      return { error: _t(keys.readFailed, { path: pathArg, msg: _t(keys.notAFile) }) };
    }
    size = st.size;
  } catch (e: any) {
    return { error: _t(keys.readFailed, { path: pathArg, msg: sanitizeForModel(e?.message || String(e)) }) };
  }
  if (size > EVAL_ITEM_MAX_BYTES) {
    return { error: _t(keys.tooLarge, { path: pathArg, kb: EVAL_ITEM_MAX_BYTES / 1024 }) };
  }
  if (ctx.usedBytes + size > totalMax) {
    return { error: _t(keys.totalTooLarge, { path: pathArg, kb: totalMax / 1024 }) };
  }
  try {
    const buf = readFileSync(abs);
    // stat 与 read 之间文件可能变大：以实际读到的字节数再判一次总量
    if (ctx.usedBytes + buf.byteLength > totalMax) {
      return { error: _t(keys.totalTooLarge, { path: pathArg, kb: totalMax / 1024 }) };
    }
    return { content: buf.toString("utf-8"), bytes: buf.byteLength };
  } catch (e: any) {
    return { error: _t(keys.readFailed, { path: pathArg, msg: sanitizeForModel(e?.message || String(e)) }) };
  }
}

/**
 * `evaluate` 多客户端项的插件侧装配：把 IO（文件读取 / 浏览器调用）注入 `runEvalItems`。
 *
 * - 文件项在执行前统一读入（累计字节数受 `EVAL_TOTAL_MAX_BYTES` 约束）：任一项失败整次不执行脚本
 * - `--continue-on-error` 决定某项失败后是否继续执行后续项
 *
 * @param a 已解析命令参数（`_sessionId` / `timeout` / `continue-on-error`）
 * @param items 有序执行项
 * @returns 报告文本（不含 timeout 提示，由调用方追加）
 */
async function runEvalCommand(a: any, items: EvalItem[]): Promise<string> {
  const workspaceDir = getAppDirectory();
  const { dirs: tmpDirs } = allowedFileDirs(a._sessionId);
  const allowedDirs = workspaceDir ? [...tmpDirs, workspaceDir] : tmpDirs;
  let usedBytes = 0;
  return runEvalItems(items, {
    readFile: (path: string): EvalFileRead => {
      const read = readEvalFile(path, { workspaceDir, allowedDirs, usedBytes });
      if (!("error" in read)) usedBytes += read.bytes;
      return read;
    },
    evaluate: (script: string) => service.evaluate(evalPayload(a, script)),
    continued: a["continue-on-error"] === true,
  });
}

/** 把 `parseArgs` 的 `multiple` 结果（单值 / 数组 / 未传）统一成非空字符串数组 */
function toStringList(v: unknown): string[] {
  if (v === undefined || v === null) return [];
  const arr = Array.isArray(v) ? v : [v];
  return arr.map((x) => String(x)).filter((s) => s.length > 0);
}

/**
 * 读取 inject 的文件项（复用 evaluate 的 containment / 体积校验，仅换文案与单规则上限）。
 *
 * @param a 已解析命令参数（用 `_sessionId` 定位会话 tmp 桶）
 * @param files `--file` 路径列表（按传入顺序）
 * @returns 读入成功的文件源列表，或可直接回模型的错误文案
 */
function readInjectFiles(a: any, files: string[]): { sources: Array<{ path: string; code: string }> } | { error: string } {
  const workspaceDir = getAppDirectory();
  const { dirs: tmpDirs } = allowedFileDirs(a._sessionId);
  const allowedDirs = workspaceDir ? [...tmpDirs, workspaceDir] : tmpDirs;
  const sources: Array<{ path: string; code: string }> = [];
  let usedBytes = 0;
  for (const path of files) {
    // 单规则上限 = items 合计上限（契约 256 KiB），故这里把「合计」直接按其判定
    const read = readEvalFile(
      path,
      { workspaceDir, allowedDirs, usedBytes, totalMaxBytes: INJECT_RULE_MAX_BYTES },
      INJECT_FILE_KEYS,
    );
    if ("error" in read) return { error: read.error };
    usedBytes += read.bytes;
    sources.push({ path, code: read.content });
  }
  return { sources };
}

/**
 * `inject add`：读文件 → 构造 items → 校验上限 → POST `/api/inject/rules`。
 *
 * - `--file` 只允许工作区目录 / 插件 tmp（与 evaluate 同 containment）；单条 ≤256 KiB
 * - 文件内容转成 `{kind:"text", code, name:"<basename>"}`，`--text` 的 name 固定 `text`；
 *   Rust 只收到内容，不接触路径
 * - `--disabled` 等价 `enabled:false`；`--order` 缺省时不下发（由 Rust 取默认）
 *
 * @param a 已解析命令参数
 * @returns 回模型的单行结果文本
 */
async function runInjectAdd(a: any): Promise<string> {
  const match = typeof a.match === "string" ? a.match.trim() : "";
  const files = toStringList(a.file);
  const texts = toStringList(a.text);
  const issue = injectAddIssue(match, files.length, texts.length);
  if (issue === "no_match") return _t("cli.err.inject_match_required");
  if (issue === "no_items") return _t("cli.err.inject_no_items");

  const read = readInjectFiles(a, files);
  if ("error" in read) return read.error;
  const items: InjectItem[] = buildInjectItems(read.sources, texts);
  const over = injectRuleLimitIssue(items);
  if (over) return _t("cli.err.inject_items_too_large", { kb: over.kb });

  const payload: Record<string, any> = { action: "add", match, items, enabled: a.disabled !== true };
  if (a.order !== undefined) payload.order = a.order;
  const data = (await service.injectRules(payload)) as InjectRulesData | undefined;
  return formatInjectAdd(data?.rule);
}

/**
 * `inject remove|enable|disable`：按 id 变更单条规则（`--id` 必填）。
 *
 * @param a 已解析命令参数
 * @param action 子动作
 * @returns 回模型的单行结果文本
 */
async function runInjectMutate(a: any, action: "remove" | "enable" | "disable"): Promise<string> {
  const id = typeof a.id === "string" ? a.id.trim() : "";
  if (!id) return _t("cli.err.inject_id_required");
  const data = (await service.injectRules({ action, id })) as InjectRulesData | undefined;
  const n = (data?.rules ?? []).length;
  const key = action === "remove" ? "cli.out.inject_removed" : action === "enable" ? "cli.out.inject_enabled" : "cli.out.inject_disabled";
  return _t(key, { id, n });
}

/**
 * `inject` 子命令分发（未给子动作时按只读的 `list` 处理）。
 *
 * @param a 已解析命令参数
 * @param positionals 位置参数（第一个是子动作）
 * @returns 回模型的文本
 */
async function runInject(a: any, positionals: string[]): Promise<string> {
  const action = normalizeInjectAction(positionals[0]);
  switch (action) {
    case "add":
      return runInjectAdd(a);
    case "list": {
      const data = (await service.injectRules({ action: "list" })) as InjectRulesData | undefined;
      return (data?.rules ?? []).length > 0 ? formatInjectList(data) : _t("cli.out.inject_empty");
    }
    case "remove":
    case "enable":
    case "disable":
      return runInjectMutate(a, action);
    case "clear":
      await service.injectRules({ action: "clear" });
      return _t("cli.out.inject_cleared");
    case "test": {
      const url = typeof a.url === "string" ? a.url.trim() : "";
      if (!url) return _t("cli.err.inject_url_required");
      return formatInjectTest(await service.injectTest({ url }));
    }
    default:
      return _t("cli.err.inject_unknown_action", { action });
  }
}

/**
 * `type`：向当前焦点插入文本（不清空），可选先聚焦 selector、插入后回车。
 *
 * `--text` 支持 `@file:<path>`：走 `evaluate` 同一套读取与 containment（工作区目录 / 插件 tmp），
 * 内容不进模型上下文。文本校验（空 / 超 8 KiB）在插件侧先做一次，避免把注定失败的请求发下去。
 *
 * @param a 已解析命令参数（`text` / `selector` / `enter` / `timeout`）
 * @returns 回模型的单行结果文本
 */
async function runType(a: any): Promise<string> {
  const tn = timeoutNote(a);
  const raw = typeof a.text === "string" ? a.text : "";
  if (!raw) return _t("cli.err.type_text_required");

  const workspaceDir = getAppDirectory();
  const { dirs: tmpDirs } = allowedFileDirs(a._sessionId);
  const allowedDirs = workspaceDir ? [...tmpDirs, workspaceDir] : tmpDirs;
  const resolved = resolveTypeText(raw, (path) =>
    readEvalFile(path, { workspaceDir, allowedDirs, usedBytes: 0 }, TYPE_FILE_KEYS),
  );
  if ("error" in resolved) return resolved.error;

  const issue = typeTextIssue(resolved.text);
  if (issue === "required") return _t("cli.err.type_text_required");
  if (issue === "too_large") return _t("cli.err.type_text_too_large", { kb: TYPE_TEXT_MAX_BYTES / 1024 });

  const selector = typeof a.selector === "string" ? a.selector : "";
  const entered = a.enter === true;
  const r = await service.typeText({
    text: resolved.text,
    selector,
    enter: entered,
    _sessionId: a._sessionId,
    ...(a.timeout !== undefined ? { timeout: a.timeout } : {}),
  });
  return formatTypeOutput(r?.inserted ?? 0, selector, entered) + tn;
}

/** console_logs 请求体：只挑契约字段（CLI 专用 flag 不下发；`--no-dedupe` 等价 `--group=none`） */
function consoleLogParams(a: any): Record<string, any> {
  const p: Record<string, any> = { _sessionId: a._sessionId };
  if (a.type !== undefined) p.type = a.type;
  if (a["no-dedupe"] === true) p.group = "none";
  else if (a.group !== undefined) p.group = a.group;
  if (a.phase !== undefined) p.phase = a.phase;
  if (a.search !== undefined) p.search = a.search;
  if (a.limit !== undefined) p.limit = a.limit;
  if (a.tab !== undefined) p.tab = a.tab;
  if (a["include-internal"] === true) p.includeInternal = true;
  if (a.clear === true) p.clear = true;
  return p;
}

/** network_logs 请求体：`--status` 纯数字转 number（契约允许 `2xx|…|N`） */
function networkLogParams(a: any): Record<string, any> {
  const p: Record<string, any> = { _sessionId: a._sessionId };
  if (a.type !== undefined) p.type = a.type;
  if (a.status !== undefined && a.status !== "") {
    p.status = /^\d+$/.test(String(a.status)) ? Number(a.status) : a.status;
  }
  if (a.failed === true) p.failed = true;
  if (a.phase !== undefined) p.phase = a.phase;
  if (a.search !== undefined) p.search = a.search;
  if (a["no-dedupe"] === true) p.group = "none";
  else if (a.group !== undefined) p.group = a.group;
  if (a.limit !== undefined) p.limit = a.limit;
  if (a.tab !== undefined) p.tab = a.tab;
  if (a["include-headers"] === true) p.includeHeaders = true;
  if (a["reveal-secrets"] === true) p.revealSecrets = true;
  if (a.clear === true) p.clear = true;
  return p;
}

const COMMANDS: Record<string, CmdDef> = {
  navigate: { usage: "--url", descKey: "cli.cmd.navigate", args: [{ flag: "url" }], run: async (a) => {
    const r = await service.navigate(a);
    if (r.installError) return _t("cli.out.navigate.install_failed", { error: r.error });
    return r.installing ? _t("cli.out.navigate.installing", { name: r.installing, progress: r.progress }) : _t("cli.out.navigate.ok", { url: r.url }) + readyNote(r);
  } },
  click: { usage: "--selector [--timeout ms]", descKey: "cli.cmd.click", args: [{ flag: "selector" }, { flag: "timeout" }], run: async (a) => { const tn = timeoutNote(a); const r = await service.click(a); return _t("cli.out.click") + degradedNote(r) + tn; } },
  fill: { usage: "--selector --value [--timeout ms]", descKey: "cli.cmd.fill", args: [{ flag: "selector" }, { flag: "value" }, { flag: "timeout" }], run: async (a) => { const tn = timeoutNote(a); const r = await service.fill(a); return _t("cli.out.fill") + degradedNote(r) + tn; } },
  clear: { usage: "--selector [--timeout ms]", descKey: "cli.cmd.clear", args: [{ flag: "selector" }, { flag: "timeout" }], run: async (a) => { const tn = timeoutNote(a); const r = await service.clear(a); return _t("cli.out.clear") + degradedNote(r) + tn; } },
  select: { usage: "--selector --value [--timeout ms]", descKey: "cli.cmd.select", args: [{ flag: "selector" }, { flag: "value" }, { flag: "timeout" }], run: async (a) => { const tn = timeoutNote(a); const r = await service.select(a); return _t("cli.out.select") + degradedNote(r) + tn; } },
  hover: { usage: "--selector [--timeout ms]", descKey: "cli.cmd.hover", args: [{ flag: "selector" }, { flag: "timeout" }], run: async (a) => { const tn = timeoutNote(a); const r = await service.hover(a); return _t("cli.out.hover") + degradedNote(r) + tn; } },
  drag: { usage: "--sourceSelector --targetSelector [--timeout ms]", descKey: "cli.cmd.drag", args: [{ flag: "sourceSelector" }, { flag: "targetSelector" }, { flag: "timeout" }], run: async (a) => { const tn = timeoutNote(a); await service.drag(a); return _t("cli.out.drag") + tn; } },
  press_key: { usage: "--key [--selector] [--timeout ms]", descKey: "cli.cmd.press_key", args: [{ flag: "key" }, { flag: "selector" }, { flag: "timeout" }], run: async (a) => { const tn = timeoutNote(a); await service.pressKey(a); return _t("cli.out.press_key", { key: a.key }) + tn; } },
  // type:向当前焦点插入文本(不清空);--text 支持 @file:<path>,--enter 插入后回车
  type: { usage: "--text <text|@file:path> [--selector] [--enter] [--timeout ms]", descKey: "cli.cmd.type", args: [{ flag: "text" }, { flag: "selector" }, { flag: "enter", type: "boolean" }, { flag: "timeout" }], run: async (a) => runType(a) },
  upload_file: { usage: "--selector --filePath [--timeout ms]", descKey: "cli.cmd.upload_file", args: [{ flag: "selector" }, { flag: "filePath" }, { flag: "timeout" }], run: async (a) => { const tn = timeoutNote(a); await service.uploadFile(a); return _t("cli.out.upload_file") + tn; } },
  screenshot: { usage: "[--selector] [--timeout ms]", descKey: "cli.cmd.screenshot", args: [{ flag: "selector" }, { flag: "timeout" }], run: async (a) => {
    const tn = timeoutNote(a);
    const r = await service.screenshot(a);
    // 元素截图:先滚动到可见再截;rect 超出视口时走越界路径(截全),滚动未稳定/未到达如实标注
    const where = a.selector ? _t("cli.out.screenshot.element", { sel: a.selector }) : _t("cli.out.screenshot.fullpage");
    const note = (r as any)?.beyondViewport ? _t("cli.note.beyond") : "";
    return { output: _t("cli.out.screenshot", { where }) + note + reachedNote(r) + stableNote(r) + tn, attachments: [{ type: "file", mime: r.mime || "image/png", url: `data:${r.mime || "image/png"};base64,${r.base64}` }] };
  } },
  // evaluate:支持「多文件 + 内联文本混合、按顺序串行执行」;只给 --script 时保持 1.12.0 的输出格式
  evaluate: {
    usage: "[--script <js>] [<js-text>|@file:<path>]... [--continue-on-error] [--timeout ms]",
    descKey: "cli.cmd.evaluate",
    args: [{ flag: "script" }, { flag: "continue-on-error", type: "boolean" }, { flag: "timeout" }],
    run: async (a, positionals) => {
      const tn = timeoutNote(a);
      // 旧行为:单项 --script(位置参数与它互斥,同时给会有顺序歧义 → 报错)
      if (a.script !== undefined && positionals.length === 0) {
        return JSON.stringify(await service.evaluate(evalPayload(a, a.script)), null, 2) + tn;
      }
      const parsed = classifyEvalArgs(positionals, a.script !== undefined);
      if (!parsed.ok) {
        return _t(parsed.reason === "script_conflict" ? "cli.err.eval_script_conflict" : "cli.err.eval_no_items");
      }
      return (await runEvalCommand(a, parsed.items)) + tn;
    },
  },
  // inject:持久注入规则(插件侧契约 + Rust 端点);子动作见 usage,add 支持重复 --file/--text
  inject: {
    usage: "[add --match <glob|re:...> [--file <path>]... [--text <code>]... [--order N] [--disabled] | list | remove|enable|disable --id <id> | clear | test --url <url>]",
    descKey: "cli.cmd.inject",
    args: [
      { flag: "match" }, { flag: "file", multiple: true }, { flag: "text", multiple: true },
      { flag: "order" }, { flag: "disabled", type: "boolean" },
      { flag: "id" }, { flag: "url" },
    ],
    run: async (a, positionals) => runInject(a, positionals),
  },
  get_visible_text: { usage: "[--selector]", descKey: "cli.cmd.get_visible_text", args: [{ flag: "selector" }], run: async (a) => (await service.visibleText(a))?.text || _t("cli.out.no_visible_text") },
  get_visible_html: { usage: "[--selector --removeScripts --removeComments --maxLength]", descKey: "cli.cmd.get_visible_html", args: [{ flag: "selector" }, { flag: "removeScripts", type: "boolean" }, { flag: "removeComments", type: "boolean" }, { flag: "maxLength" }], run: async (a) => {
    // 宿主要求 tool 返回字符串;服务端返回对象 {html,truncated},这里取出 html 文本本体
    const r = await service.visibleHtml(a);
    const html = r?.html || "";
    if (!html) return _t("cli.out.no_visible_html");
    // 被截断时追加提示:html.length 即服务端实际生效的 maxLength(已 slice),无需再传参
    return r?.truncated ? `${html}\n${_t("cli.out.html_truncated", { n: html.length })}` : html;
  } },
  console_logs: {
    usage: "[--type --group burst|none --phase --search --limit --tab --include-internal --clear --no-dedupe]",
    descKey: "cli.cmd.console_logs",
    args: [
      { flag: "type" }, { flag: "group" }, { flag: "phase" }, { flag: "search" }, { flag: "limit" },
      { flag: "tab" }, { flag: "include-internal", type: "boolean" },
      { flag: "clear", type: "boolean" }, { flag: "no-dedupe", type: "boolean" },
    ],
    run: async (a) => {
      const r = await service.consoleLogs(consoleLogParams(a)) as LogStreamData<ConsoleLogItem> | undefined;
      return renderLogText(formatConsoleItems(r?.items), r?.dropped) || _t("cli.out.console_empty");
    },
  },
  network_logs: {
    usage: "[--type --status 2xx|3xx|4xx|5xx|N --failed --search --group burst|key|none --phase --limit --tab --include-headers --reveal-secrets --clear --no-dedupe]",
    descKey: "cli.cmd.network_logs",
    args: [
      { flag: "type" }, { flag: "status" }, { flag: "failed", type: "boolean" }, { flag: "search" }, { flag: "phase" },
      { flag: "group" }, { flag: "limit" }, { flag: "tab" },
      { flag: "include-headers", type: "boolean" }, { flag: "reveal-secrets", type: "boolean" },
      { flag: "clear", type: "boolean" }, { flag: "no-dedupe", type: "boolean" },
    ],
    run: async (a) => {
      const r = await service.networkLogs(networkLogParams(a)) as LogStreamData<NetworkLogItem> | undefined;
      const lines = formatNetworkItems(r?.items, { includeHeaders: a["include-headers"] === true });
      return renderLogText(lines, r?.dropped) || _t("cli.out.network_empty");
    },
  },
  go_back: { usage: "", descKey: "cli.cmd.go_back", run: async (a) => { const r = await service.goBack(sidArgs(a)); return _t("cli.out.go_back", { url: r.url }) + readyNote(r); } },
  go_forward: { usage: "", descKey: "cli.cmd.go_forward", run: async (a) => { const r = await service.goForward(sidArgs(a)); return _t("cli.out.go_forward", { url: r.url }) + readyNote(r); } },
  // resize:宽高=内容区(视口)尺寸;被钳到下限时如实标注(服务端返回 clamped/content/window)
  resize: { usage: "--width --height", descKey: "cli.cmd.resize", args: [{ flag: "width" }, { flag: "height" }], run: async (a) => {
    const r = await service.resize(a);
    const c = r.content || {};
    return _t("cli.out.resize", { w: c.w, h: c.h }) + (r.clamped ? _t("cli.out.resize_clamped") : "");
  } },
  set_device: { usage: "[--name]", descKey: "cli.cmd.set_device", args: [{ flag: "name" }], run: async (a) => {
    if (!a.name) {
      const list = await service.deviceList(sidArgs(a));
      return [_t("cli.out.device_list_header"), "", ...(list.devices || []).map((d: any) => `  ${d.name}  ${d.width}x${d.height}${d.ua ? `  ${d.ua.slice(0, 60)}...` : "  (default UA)"}`)].join("\n");
    }
    const r = await service.device({ name: a.name, ...sidArgs(a) });
    return _t("cli.out.device_applied", { name: r.device, w: r.width, h: r.height, ua: r.ua });
  } },
  devtools: { usage: "[--action]", descKey: "cli.cmd.devtools", args: [{ flag: "action" }], run: async (a) => {
    const r = await service.devtools({ action: a.action || "toggle", ...sidArgs(a) });
    // supported=false:该平台无法真正关闭(Windows/wry 空实现)→ 如实提示,不接受误导性的「已关闭」
    if (r?.supported === false) return _t("cli.out.devtools_unsupported");
    return r.open ? _t("cli.out.devtools_open") : _t("cli.out.devtools_closed");
  } },
  reload: { usage: "", descKey: "cli.cmd.reload", run: async (a) => { const r = await service.reload(sidArgs(a)); return r?.url ? _t("cli.out.reload", { url: r.url }) : _t("cli.out.reload_nourl"); } },
  open_window: { usage: "", descKey: "cli.cmd.open_window", run: async (a) => { await openWindow(sidOf(a)); return _t("cli.out.open_window"); } },
  close: { usage: "", descKey: "cli.cmd.close", run: async (a) => {
    const sid = sidOf(a);
    // 运行中或**启动中**都算「有实例可关」:启动中的实例由 closeSession 置 closedSids,启动链随后自毁
    if (!isRunning(sid) && !isStarting(sid)) return _t("cli.out.close_already");
    await closeSession(sid);
    return _t("cli.out.close_done");
  } },
  show_notification: { usage: "--message [--type]", descKey: "cli.cmd.show_notification", args: [{ flag: "message" }, { flag: "type" }], run: async (a) => { await service.notify(a); return _t("cli.out.notify_shown"); } },
  scroll: { usage: "[--direction --amount]", descKey: "cli.cmd.scroll", args: [{ flag: "direction" }, { flag: "amount" }], run: async (a) => { const r = await service.scroll(a); return _t("cli.out.scroll", { dir: a.direction || "down", n: a.amount || 300 }) + stableNote(r); } },
  wait_for_selector: { usage: "--selector [--timeout ms]", descKey: "cli.cmd.wait_for_selector", args: [{ flag: "selector" }, { flag: "timeout" }], run: async (a) => { const tn = timeoutNote(a); await service.waitForSelector(a); return _t("cli.out.wait_appeared") + tn; } },
  click_and_switch_tab: { usage: "--selector", descKey: "cli.cmd.click_and_switch_tab", args: [{ flag: "selector" }], run: async (a) => { const r = await service.clickSwitchTab(a); return _t("cli.out.click_switch_tab", { sel: a.selector, url: r.url || "" }) + degradedNote(r); } },
  iframe_click: { usage: "--iframeSelector --selector [--timeout ms]", descKey: "cli.cmd.iframe_click", args: [{ flag: "iframeSelector" }, { flag: "selector" }, { flag: "timeout" }], run: async (a) => { const tn = timeoutNote(a); const r = await service.iframeClick(a); return _t("cli.out.iframe_click") + degradedNote(r) + tn; } },
  iframe_fill: { usage: "--iframeSelector --selector --value [--timeout ms]", descKey: "cli.cmd.iframe_fill", args: [{ flag: "iframeSelector" }, { flag: "selector" }, { flag: "value" }, { flag: "timeout" }], run: async (a) => { const tn = timeoutNote(a); const r = await service.iframeFill(a); return _t("cli.out.iframe_fill") + degradedNote(r) + tn; } },
  get_browser_status: { usage: "", descKey: "cli.cmd.get_browser_status", run: async (a) => {
    // 纯查询:不 spawn、不懒启动(未运行只提示,不弹窗)
    const s = await probeStatus(1500, sidOf(a));
    if (!s) {
      const b = getBinaryStatus();
      // bt-shell 状态文案由 describeBinaryStatus 提供(已保证不含本机绝对路径)
      return _t("cli.out.status.not_running", { bin: describeBinaryStatus(b) });
    }
    if (s.installing && Object.keys(s.installing).length > 0) return _t("cli.out.status.installing", { list: Object.entries(s.installing).map(([b, p]) => `${b} (${p})`).join(", ") });
    if (!s.open) return _t("cli.out.status.not_open");
    return _t("cli.out.status.open", { title: s.title, url: s.url, n: s.tabs });
  } },
  list_tabs: { usage: "", descKey: "cli.cmd.list_tabs", run: async (a) => { const res = await service.tabs(sidArgs(a)); const list = res?.tabs || []; if (!list.length) return _t("cli.out.tabs_none"); return `${_t("cli.out.tabs_header", { n: list.length })}\n${list.map((p: any, i: number) => `[${i}] ${p.url || p.title}`).join("\n")}`; } },
  switch_tab: { usage: "--index", descKey: "cli.cmd.switch_tab", args: [{ flag: "index" }], run: async (a) => { const r = await service.switchTab(a); return _t("cli.out.switch_tab", { i: a.index, url: r.url }); } },
  new_tab: { usage: "--url", descKey: "cli.cmd.new_tab", args: [{ flag: "url" }], run: async (a) => { const r = await service.newTab(a); return _t("cli.out.new_tab", { url: r.url }); } },
  close_tab: { usage: "[--index]", descKey: "cli.cmd.close_tab", args: [{ flag: "index" }], run: async (a) => { await service.closeTab(a); return a.index !== undefined ? _t("cli.out.close_tab", { i: a.index }) : _t("cli.out.close_tab_current"); } },
  get_element_state: { usage: "--selector [--timeout ms]", descKey: "cli.cmd.get_element_state", args: [{ flag: "selector" }, { flag: "timeout" }], run: async (a) => { const tn = timeoutNote(a); const r: any = await service.elementState(a); if (!r || !r.exists) return _t("cli.out.element_not_found", { sel: a.selector }); const text = r.text ? `\nText: ${r.text}` : ""; return _t("cli.out.element_state", { tag: r.tag, sel: a.selector, visible: r.visible, text, rect: `${r.rect.x},${r.rect.y} ${r.rect.w}x${r.rect.h}` }) + tn; } },
  scroll_to_element: { usage: "--selector [--timeout ms]", descKey: "cli.cmd.scroll_to_element", args: [{ flag: "selector" }, { flag: "timeout" }], run: async (a) => { const tn = timeoutNote(a); const r = await service.scrollToElement(a); return _t("cli.out.scroll_to_element") + reachedNote(r) + tn; } },
  get_dropdown_options: { usage: "--selector [--timeout ms]", descKey: "cli.cmd.get_dropdown_options", args: [{ flag: "selector" }, { flag: "timeout" }], run: async (a) => { const tn = timeoutNote(a); const r: any = await service.dropdownOptions(a); const opts: any[] = r?.options ?? []; if (!opts.length) return _t("cli.out.dropdown_none", { sel: a.selector }); return opts.map((o: any) => `${o.selected ? "* " : "  "}${o.value}: ${o.text}`).join("\n") + tn; } },
  custom_user_agent: { usage: "--userAgent", descKey: "cli.cmd.custom_user_agent", args: [{ flag: "userAgent" }], run: async (a) => { await service.userAgent(a); return _t("cli.out.ua_set"); } },
  expect_response: { usage: "--url", descKey: "cli.cmd.expect_response", args: [{ flag: "url" }], run: async (a) => { await service.expectResponse({ urlPattern: a.url, ...sidArgs(a) }); return _t("cli.out.expect_response", { url: a.url }); } },
  assert_response: { usage: "--id", descKey: "cli.cmd.assert_response", args: [{ flag: "id" }], run: async (a) => { const r = await service.assertResponse({ id: a.id, ...sidArgs(a) }); return r.matched ? _t("cli.out.assert_matched", { url: r.url, status: r.status }) : r.error || _t("cli.out.assert_none"); } },
  get_accessibility_tree: { usage: "[--selector --maxDepth] [--timeout ms]", descKey: "cli.cmd.get_accessibility_tree", args: [{ flag: "selector" }, { flag: "maxDepth" }, { flag: "timeout" }], run: async (a) => { const tn = timeoutNote(a); const s = await service.accessibility(a); return (s ? formatNode(s, 0, a.maxDepth ?? 8) : _t("cli.out.no_ax")) + tn; } },
  list_records: { usage: "", descKey: "cli.cmd.list_records", run: async (a) => { try { const records = await service.annotateRecords(sidArgs(a)); if (!records || !records.length) return _t("cli.out.no_records"); return records.map((r: any) => { const rect = r.rect ? ` rect=(${r.rect[0]},${r.rect[1]},${r.rect[2]},${r.rect[3]})` : ""; const img = r.image ? " +img" : ""; const sent = r.sent ? " [sent]" : ""; return `[${r.index}] ${r.selector}${rect}${img}${sent}${r.note ? ` note="${r.note}"` : ""}`; }).join("\n"); } catch { return _t("cli.out.no_records"); } } },
  read_record_content: { usage: "--id", descKey: "cli.cmd.read_record_content", args: [{ flag: "id" }], run: async (a) => { try { const records = await service.annotateRecords({ ...sidArgs(a), includeImages: true }); const rec = (records || []).find((r: any) => r.index === a.id); if (!rec) return _t("cli.out.record_not_found", { id: a.id }); let img = ""; if (rec.image) { const p = saveShotImage(log, sidOf(a), rec.index, rec.image); const kb = Math.round(rec.image.length / 1024); img = p ? _t("cli.out.record_image_saved", { kb, path: p }) : _t("cli.out.record_image", { kb }); } return `#${rec.index} ${rec.selector}\nrect: ${JSON.stringify(rec.rect)}${img}\nnote: ${rec.note || _t("cli.out.record_note_none")}`; } catch { return _t("cli.out.no_records"); } } },
  fake_audio: { usage: "--kind [--data --freq --durMs --notes --digits --loop]", descKey: "cli.cmd.fake_audio", args: [{ flag: "kind" }, { flag: "data" }, { flag: "freq" }, { flag: "durMs" }, { flag: "notes" }, { flag: "digits" }, { flag: "loop", type: "boolean" }], run: async (a) => { const r = await service.mediaAudio(a); return _t("cli.out.fake_audio", { injected: r.injected }); } },
  fake_video: { usage: "--kind [--data --url --loop]", descKey: "cli.cmd.fake_video", args: [{ flag: "kind" }, { flag: "data" }, { flag: "url" }, { flag: "loop", type: "boolean" }], run: async (a) => { const r = await service.mediaVideo(a); return _t("cli.out.fake_video", { injected: r.injected }); } },
  profile: { usage: "[--set <name> | --delete <name>]", descKey: "cli.cmd.profile", args: [{ flag: "set" }, { flag: "delete" }], run: async (a) => {
    const active = getActiveProfile();
    /** 解析 profile 路径;名字非法时返回错误文案(不 mkdir、不 rm) */
    const resolveOrError = (raw: any): { dir?: string; name: string; err?: string } => {
      const name = String(raw);
      try {
        return { dir: resolveProfilePath(name), name };
      } catch {
        return { name, err: _t("cli.err.profile_invalid_name", { name }) };
      }
    };
    if (a.delete) {
      const r = resolveOrError(a.delete);
      if (r.err) return r.err;
      const name = r.name;
      if (name === "default") return _t("cli.out.profile_no_delete_default");
      if (name === active) return _t("cli.out.profile_no_delete_active");
      const dir = r.dir!;
      if (!existsSync(dir)) return _t("cli.out.profile_not_found", { name });
      rmSync(dir, { recursive: true, force: true });
      return _t("cli.out.profile_deleted", { name });
    }
    if (a.set) {
      const r = resolveOrError(a.set);
      if (r.err) return r.err;
      const name = r.name;
      // 目标配置目录不存在则创建(允许 "profile --set <新名>" 直接新建并切换)
      mkdirSync(r.dir!, { recursive: true });
      setActiveProfile(name);
      // 1.9.0 会话隔离:profile 作用域改为「影响**后续新会话**的数据根(sessions/<profile>/<sid>)」。
      // **不再重启共享 shell** —— 运行中的会话各自持有独立实例,切换互不打扰(旧实现会杀掉全局唯一窗口)。
      return _t("cli.out.profile_switched", { name });
    }
    // 列出 user-data 下的配置目录
    mkdirSync(getProfilesDir(), { recursive: true });
    const dirs = readdirSync(getProfilesDir(), { withFileTypes: true })
      .filter((d) => d.isDirectory())
      .map((d) => d.name);
    const list = dirs.map((d) => `${d === active ? "*" : " "} ${d}`).join("\n") || _t("cli.out.profiles_none");
    return _t("cli.out.profiles_header", { list, active });
  } },
  exports: { usage: "", descKey: "cli.cmd.exports", run: async () => {
    const dir = getExportsDir();
    mkdirSync(dir, { recursive: true });
    return _t("cli.out.exports", { dir });
  } },
  lookup: { usage: "--csv <path> --url <site>", descKey: "cli.cmd.lookup", args: [{ flag: "csv" }, { flag: "url" }], run: async (a) => {
    const csv = a.csv;
    const url = a.url;
    if (!csv || !url) return _t("cli.out.lookup_usage");
    // 解析密码 CSV(name,url,username,password[,note]),按 url 子串匹配
    const lines = readFileSync(csv, "utf-8").split(/\r?\n/).filter((l) => l.trim());
    for (const line of lines.slice(1)) {
      const cols = splitCsvLine(line);
      if (cols.length < 4) continue;
      const rowUrl = cols[1] || "";
      if (rowUrl.toLowerCase().includes(String(url).toLowerCase())) {
        const username = cols[2] || "";
        const password = cols[3] || "";
        // 密码写入临时文件,返回 @file: 引用(不进上下文);用完即删
        const tmpDir = getTmpDir();
        mkdirSync(tmpDir, { recursive: true });
        const tmp = resolve(tmpDir, `secret-${Date.now()}-${Math.random().toString(36).slice(2, 8)}.tmp`);
        writeFileSync(tmp, password, "utf-8");
        return _t("cli.out.lookup_found", { user: username, url: rowUrl, tmp });
      }
    }
    return _t("cli.out.lookup_none", { url });
  } },
};

/** 帮助全文(command=help 或未知命令时返回;命令描述与结构按 toolLang 本地化) */
function buildHelp(): string {
  const lines = Object.entries(COMMANDS).map(
    ([k, v]) => `  ${k.padEnd(22)} ${v.usage.padEnd(42)} ${_t(v.descKey)}`,
  );
  const examples = [_t("cli.example.1"), _t("cli.example.2"), _t("cli.example.3"), _t("cli.example.4"), _t("cli.example.5")]
    .map((e) => `  ${e}`)
    .join("\n");
  return `${_t("cli.header")}\n\n${_t("cli.usage")}\n\n${_t("cli.usage.flags")}\n\n${_t("cli.commands")}\n${lines.join("\n")}\n\n${_t("cli.examples")}\n${examples}`;
}

/** 描述内嵌的紧凑命令索引(模型无需先调 help 即可用) */
const DESC_INDEX = Object.entries(COMMANDS).map(([k, v]) => `${k} ${v.usage}`).join(" | ");

/** 解析 --flag 风格参数(基于 node:util.parseArgs,未知 flag 报错;tokens 已由 string-argv 正确分词)
 * 全局 flag `--private-session` 对所有命令可用,由调用方消费(不进入命令业务参数)。
 *
 * 位置参数不再丢弃:返回 `{ values, positionals }`,供 `evaluate` 按顺序执行多客户端项使用;
 * 其它命令沿用旧行为(位置参数不参与执行)。
 *
 * `parseArgs` 默认把值解析成字符串,而 Rust 端对下列 flag 用 `as_i64`/`as_f64` 读取
 * (取不到就落默认值/0):不转换会静默失效(如 `resize --width 800` 被钳到下限)。
 * 因此这里对已知数值 flag 统一转 number;非数字直接报错,不带着坏值进浏览器。 */
function parseFlagArgs(tokens: string[], def: CmdDef): { values: Record<string, any>; positionals: string[] } {
  if (tokens.length === 0) return { values: {}, positionals: [] };
  // 全局 boolean flag 先声明,避免被当作未知参数;若它出现在某个 string flag 之后,
  // parseArgs 会把它当成该 flag 的值(与用户显式传值一致,不会误判为私有会话)。
  const options: Record<string, any> = { "private-session": { type: "boolean" } };
  for (const arg of def.args || []) options[arg.flag] = { type: arg.type || "string" };
  for (const arg of def.args || []) if (arg.multiple) options[arg.flag].multiple = true;
  const { values, positionals } = parseArgs({ args: tokens, options, allowPositionals: true });
  for (const key of Object.keys(values)) {
    if (!NUMERIC_FLAGS.has(key)) continue;
    const raw = values[key];
    if (raw === undefined || raw === null || raw === "") continue;
    const n = Number(String(raw).trim());
    if (!Number.isFinite(n)) {
      throw new Error(_t("cli.err.flag_not_number", { flag: key, v: String(raw) }));
    }
    (values as Record<string, any>)[key] = n;
  }
  return { values, positionals };
}

/** 需要转成 number 的 flag(Rust 端按数值读取;新增数值参数时必须同步登记) */
const NUMERIC_FLAGS = new Set(["width", "height", "index", "freq", "durMs", "limit", "tab", "order"]);

/** 简单 CSV 行解析(处理引号包裹的字段,Edge/Chrome 密码导出格式) */
function splitCsvLine(line: string): string[] {
  const out: string[] = [];
  let cur = "";
  let inQ = false;
  for (let i = 0; i < line.length; i++) {
    const ch = line[i];
    if (inQ) {
      if (ch === '"') {
        if (line[i + 1] === '"') { cur += '"'; i++; } else inQ = false;
      } else cur += ch;
    } else if (ch === '"') inQ = true;
    else if (ch === ",") { out.push(cur); cur = ""; }
    else cur += ch;
  }
  out.push(cur);
  return out;
}

export const opencodeBrowserTool: Plugin = async ({ client, worktree, directory }) => {
  // 供 resolveShellBinary 定位本地 Rust 构建(publish-local 到缓存目录后仍能找到仓库)
  setAppDirectory(directory || "");
  registerLocale("en", en);
  registerLocale("zh", zh);
  loadConfig(worktree);
  const config = getConfig();

  try {
    log.loaded();
    // 懒启动:仅保存启动参数,不立即弹窗;open_window 或首次调工具时启动
    // 1.9.0 会话隔离:每个会话独立实例(独立端口 + 独立 user-data-dir),由 client.ts 的注册表管理
    configureService({
      nodePath: config.nodePath || "",
      browsersPath: getBrowsersDir(),
      // 会话数据目录:sessions/<profile>/<sid>(profile 影响后续新会话的数据根)
      resolveDataDir: (sid: string) => getSessionDataDir(sid),
    });
    // 会话树归并的 client:父会话查询用 client.session.get({ path: { id } }).parentID
    sessionTreeClient = client;
    // 会话 id 兜底解析器:工具调用没带 _sessionId 时用钩子缓存;若该 raw 已解析过根,
    // 返回归并后的 key(否则未显式透传 sid 的调用会落到子会话,另起实例)
    setSessionIdResolver(() => sessionTree.knownKey(sessionCache.get()));
    // 可选会话标题解析器:client.session.get 之类;拿不到/超时回退 sid 后缀,不阻塞启动
    setTitleResolver(async (sid: string) => {
      try {
        const r = await client.session.get({ path: { id: sid } });
        return r?.data?.title || null;
      } catch {
        return null;
      }
    });

    // 启动即后台下载 bt-shell(fire-and-forget:不阻塞启动,失败不抛出;状态见 getBinaryStatus)
    startDownload();

    // 事件驱动投递:插件本地监听 shell 通知(推送式,替代轮询)
    setupAnnotateDelivery(client);

    return {
      event: async ({ event }) => {
        // 缓存最近会话 id:这是投递批注最可靠的 sessionID 来源
        if (event.type === "session.deleted") {
          // 删除会话(1.9.0 语义升级 + 会话树归并):清钩子缓存 + **回收该会话自己的浏览器实例**。
          // 仍是「绝不因删除而 spawn」:closeSession 只在注册表里有该 sid 时才动进程,
          // 没有实例就直接返回(不会再走 callApi → ensureInstance 那条会弹窗的老路)。
          // 归并语义:删除**子会话**时不得关闭根实例、不得删根数据目录(子会话与根共用一份)。
          const delId = extractSessionIDFromEvent(event);
          if (delId) {
            // 先取归并 key(删除后 session.get 可能失败 → 回退 raw;此前工具调用已缓存根)
            const key = await browserKeyOf(delId);
            sessionCache.clearIf(delId);
            sessionTree.forget(delId);
            if (key === delId) {
              // 根会话 / 无父会话 / 私有会话:回收它自己的实例与数据目录
              await closeSession(delId);
              clearClosedState(delId);
              // 清该 sid 的附着 notify 记录(附着模式共享 shell,避免残留 key 无限增长)
              clearAttachNotifyForSid(delId);
              // 回收该会话的数据目录(P1:否则退出后磁盘目录永不释放)
              removeSessionDataDir(delId);
              sessionCache.clearIf(key);
            } else {
              // 子会话:只清它自己的关闭标记/附着记录;根实例与根数据目录保持不动
              clearClosedState(delId);
              clearAttachNotifyForSid(delId);
              log.info(`[session] child session deleted sid=${delId} (browserKey=${key}); root instance/data kept`);
            }
          }
          return;
        }
        sessionCache.updateFromEvent(event);
      },
      // 用户发消息 / 调工具时刷新当前会话 id(参考项目用 ctx.sessionID,此处为等价钩子来源)
      "chat.message": async (input: any) => {
        sessionCache.updateFromHookInput(input);
      },
      "tool.execute.before": async (input: any) => {
        sessionCache.updateFromHookInput(input);
      },
      tool: filterDisabled(
        {
          bt_cli: tool({
            description: _t("tool.cli.desc") + "\n" + DESC_INDEX + "\n" + _t("cli.usage.flags"),
            args: {
              // 单字符串命令行: "<command> [--flag value ...]", 如 "navigate --url https://..." / "help"
              args: tool.schema.string().optional().describe(_t("tool.cli.arg.args")),
            },
            async execute(a: any, context: any) {
              // 单字符串命令行: "<command> [--flag value ...]",如 "navigate --url https://..."
              const raw = (typeof a?.args === "string" ? a.args : "").trim();
              if (!raw || raw === "help" || raw === "--help" || raw === "-h") return buildHelp();
              // string-argv 正确分词(处理引号/转义),命令名取第一个 token
              const tokens = stringArgv(raw);
              const cmdName = tokens.shift();
              if (!cmdName) return buildHelp();
              const def = COMMANDS[cmdName];
              if (!def) return `${_t("cli.unknown").replace("{cmd}", cmdName)}\n\n${buildHelp()}`;
              // 剩余 token 交给 parseArgs 解析(未知 flag 会抛错,返回用法提示自愈)
              let params: Record<string, any> = {};
              let positionals: string[] = [];
              if (tokens.length > 0) {
                try {
                  const parsed = parseFlagArgs(tokens, def);
                  params = parsed.values;
                  positionals = parsed.positionals;
                } catch (e: any) {
                  // 解析错误可能夹带底层异常文本:进模型前先脱敏(用法提示本身不含路径)
                  return `${_t("cli.invalid_args").replace("{cmd}", cmdName).replace("{msg}", sanitizeForModel(e?.message || String(e)))}\n\nUsage: bt_cli({ args: "${cmdName} ${def.usage}" })`;
                }
              }
              // 全局 flag:`--private-session` 表示本次与后续调用都用调用者自己的 sid(不归并根会话)。
              // parseFlagArgs 已声明该 boolean flag;这里消费掉,不进入命令业务参数。
              const privateRequested = params["private-session"] === true;
              delete params["private-session"];
              // 刷新钩子缓存(兜底:即使钩子未触发也能拿到 sid),再按会话树取浏览器 key:
              // raw = 调用者会话;browserKey = 私有会话?raw : 沿 parentID 回溯到的根会话。
              sessionCache.updateFromHookInput(context);
              const rawSid = resolveSessionId(context?.sessionID || "");
              if (privateRequested) sessionTree.markPrivate(rawSid);
              const browserSid = await browserKeyOf(rawSid);
              // @file: 引用解析:值以 @file:<path> 开头时读文件内容作为实际值
              // (密码等敏感值不进 LLM 上下文,如 fill --value @file:C:\...\secret.tmp)
              // 安全边界:只允许读全局 tmp(getTmpDir)或**该会话(归并后 key)** tmp(getSessionTmpDir)之内的文件;
              // 其余路径一律拒绝(不读取),避免模型借助 @file: 读取任意本地文件。
              // 允许目录由 allowedFileDirs 统一计算(其中做了 sid 形态校验,非法 sid 回落 default 桶)。
              const { globalTmpDir, dirs: allowedTmpDirs } = allowedFileDirs(browserSid);
              for (const key of Object.keys(params)) {
                const v = params[key];
                if (typeof v === "string" && v.startsWith("@file:")) {
                  // `type` 的 `--text` 自带 evaluate 同款解析（允许工作区目录 + 插件 tmp，且不删文件），
                  // 故跳过这里的「仅插件 tmp」通用层，避免两套规则叠加（通用层还会删掉全局 tmp 文件）。
                  if (cmdName === "type" && key === "text") continue;
                  const p = v.slice(6);
                  const abs = resolve(p);
                  if (!allowedTmpDirs.some((d) => isInsideDir(abs, d))) {
                    return _t("cli.err.file_outside_tmp", { path: p });
                  }
                  try {
                    params[key] = readFileSync(abs, "utf-8").replace(/\r?\n$/, "");
                    // 用完即删(仅限 lookup 生成的全局 tmp 临时密码文件;会话 tmp 内的截图等保留)
                    if (isInsideDir(abs, globalTmpDir)) {
                      unlinkSync(abs);
                    }
                  } catch (e: any) {
                    // {path} 是模型自己传入的值,保持原样;{msg} 来自 fs 错误,必须脱敏
                    return _t("cli.read_file_failed", { path: p, msg: sanitizeForModel(e?.message || String(e)) });
                  }
                }
              }
              // 透传归并后的会话 key(会话隔离 + 子会话归并):后续 sidOf/sidArgs 优先用它
              setAppDirectory(context?.directory || "");
              params._sessionId = browserSid;
              try {
                return await def.run(params, positionals);
              } catch (e: any) {
                // 命令失败可能带 fs/网络错误文本:统一脱敏后再回模型
                return `${_t("cli.failed").replace("{cmd}", cmdName).replace("{msg}", sanitizeForModel(e?.message || String(e)))}`;
              }
            },
          }),
          // 切真实设备属敏感操作,单独保留工具以维持 ask 权限
          bt_set_media_mode: tool({
            description: _t("tool.set_media_mode.desc"),
            args: {
              mode: tool.schema.enum(["simulate", "real"]).optional().describe(_t("tool.set_media_mode.arg.mode")),
            },
            async execute(a: any, context: any) {
              sessionCache.updateFromHookInput(context);
              setAppDirectory(context?.directory || "");
              // 与 bt_cli 同口径:走归并后的 key(私有会话粘住自身)
              const rawSid = resolveSessionId(context?.sessionID || "");
              const key = await browserKeyOf(rawSid);
              const r = await service.mediaMode({ ...a, _sessionId: key });
              return _t("cli.out.media_mode", { mode: r.mode });
            },
          }),
        },
        config.disabledTools,
      ),
      // 插件卸载/进程退出:优雅关闭全部会话实例(按注册表 PID,绝不按名;附着模式不动外部进程)
      dispose: async () => {
        await closeAllSessions();
      },
    };
  } catch (err) {
    log.error("Init failed", err as Error);
    throw err;
  }
};

function formatNode(node: any, depth: number, maxDepth: number): string {
  if (depth > maxDepth) return "";
  const indent = "│ ".repeat(depth);
  const parts = [indent + node.role];
  if (node.name) parts.push(`"${node.name}"`);
  if (node.value !== undefined && node.value !== "") parts.push(`= ${node.value}`);
  if (node.description) parts.push(`(${node.description})`);
  const line = parts.join(" ");
  if (node.children && node.children.length > 0) {
    const children = node.children
      .map((c: any) => formatNode(c, depth + 1, maxDepth))
      .filter(Boolean)
      .join("\n");
    return children ? line + "\n" + children : line;
  }
  return line;
}

function filterDisabled(tools: Record<string, any>, disabled?: string[]): Record<string, any> {
  if (!disabled || disabled.length === 0) return tools;
  const result: Record<string, any> = {};
  for (const [name, def] of Object.entries(tools)) {
    if (!disabled.includes(name)) result[name] = def;
  }
  return result;
}

/** 事件驱动投递:shell 面板"发送全部"/截图"发送"后 POST 插件本地 /notify/<sid> 触发 drain
 * 注意:插件函数会随会话/切目录多次执行,notify server 与 drain 状态必须进程级单例,
 * 否则同一进程会累积多个监听/重复投递(server 与 TUI 进程各一份,互不共享)
 */
let annotateClient: any = null;
/** drain 进行中(按 sid 隔离:会话 A 的 drain 不会挡住会话 B 的投递) */
const drainingSid = new Set<string>();
/** 最近一次见到的会话 id 缓存(来自 event/chat.message/tool.execute.before 钩子) */
const sessionCache = createSessionIDCache();

/** 会话树归并所需的插件 client(插件初始化时赋值;未就绪时父查询按失败处理) */
let sessionTreeClient: any = null;
/**
 * 会话树归并:子代理会话默认归并到根会话(共用实例/数据目录/标题);
 * `--private-session` 的会话粘住自身。缓存与私有标记是**进程级**的,插件重载不丢。
 */
const sessionTree = createSessionTree({
  fetchParent: async (sid: string) => {
    if (!sessionTreeClient) return undefined;
    const r = await sessionTreeClient.session.get({ path: { id: sid } });
    // 包络异常(有 error / 无 data)按失败处理:绝不把子会话误判为根并长期缓存
    if (!r || !r.data) return undefined;
    const parent = r.data.parentID;
    return typeof parent === "string" && parent.trim() ? parent.trim() : null;
  },
  onWarn: (m: string) => log.info(m),
});

/** 把截图 base64 落盘为临时 PNG,返回路径(失败返回 null);用于发送时保留图片与给出可读路径
 * 1.9.0:落盘到**该会话**的临时目录 sessions/<profile>/<sid>/tmp(不再与其它会话共享全局 tmp) */
function saveShotImage(log: any, sid: string, index: number, b64: string): string | null {
  try {
    const dir = resolve(getSessionTmpDir(sid));
    mkdirSync(dir, { recursive: true });
    const p = resolve(dir, `shot-${Date.now()}-${index}.png`);
    writeFileSync(p, Buffer.from(b64, "base64"));
    return p;
  } catch (e) {
    log.error("save screenshot image failed", e as Error);
    return null;
  }
}

/** 合法会话 id 形态(opencode 生成的 sid);不匹配则不删目录,避免意外路径 */
const SESSION_SID_RE = /^[A-Za-z0-9_-]{1,128}$/;
/** 会话目录删除失败后的重试延迟(ms;Windows 上文件常被短暂占用) */
const SESSION_DIR_RETRY_MS = 800;

/** 同步删除某 sid 的数据目录(失败仅记录,由调用方决定是否重试) */
function rmSessionDir(dir: string): boolean {
  try {
    rmSync(dir, { recursive: true, force: true });
    log.info(`[session] removed data dir for deleted session: ${dir}`);
    return true;
  } catch (e) {
    log.error(`[session] failed to remove data dir ${dir}: ${(e as Error).message}`, e as Error);
    return false;
  }
}

/**
 * 会话删除后回收**所有 profile 下**属于该 sid 的数据目录(sessions/<profile>/<sid>)。
 * 用户切换过 activeProfile 时,老 profile 目录里的同名会话也要清掉,故枚举 sessions 根的直接子目录。
 * 约束:sid 必须合法;每个目标必须在 getSessionsDir() 之内,且不等于 sessions 根或 profile 根
 *      (只删名为 <sid> 的目录,profile 目录本身永不删除)。
 * Windows 文件锁导致失败时 800ms 后对失败列表重试一次;全部成功则不设定时器。
 */
function removeSessionDataDir(sid: string): void {
  if (!SESSION_SID_RE.test(sid)) {
    log.error(`[session] skip data dir cleanup: invalid session id (${sid.length} chars)`);
    return;
  }
  const sessionsDir: string = getSessionsDir();
  // 枚举直接子目录:每个 = 一个 profile;读不到(根不存在/无权限)视为无可回收目录
  let profileNames: string[];
  try {
    profileNames = readdirSync(sessionsDir, { withFileTypes: true })
      .filter((e) => e.isDirectory())
      .map((e) => e.name);
  } catch {
    return;
  }
  const targets: string[] = [];
  for (const name of profileNames) {
    const profileDir: string = resolve(sessionsDir, name);
    const target: string = resolve(profileDir, sid);
    // 路径断言:仍在 sessions 根内,且不是 sessions 根/profile 根本身
    if (!isInsideDir(target, sessionsDir) || target === sessionsDir || target === profileDir) continue;
    if (!existsSync(target)) continue;
    targets.push(target);
  }
  if (!targets.length) return;
  const failed: string[] = targets.filter((t) => !rmSessionDir(t));
  if (!failed.length) {
    log.info(`[session] cleaned data dir for sid=${sid} across ${targets.length} profile dir(s)`);
    return;
  }
  const timer = setTimeout(() => {
    const stillFailing: string[] = failed.filter((t) => !rmSessionDir(t));
    if (stillFailing.length) {
      log.error(`[session] data dir cleanup still failing after retry (sid=${sid}): ${stillFailing.length} dir(s)`);
    }
  }, SESSION_DIR_RETRY_MS);
  // 不让重试定时器拖住进程退出
  (timer as any).unref?.();
}

/** 事件驱动投递:插件本地监听 shell 通知(收到即 drain),不再 setInterval 轮询 */
function setupAnnotateDelivery(client: any): void {
  // 更新 client 引用(插件重载后使用最新 client)
  annotateClient = client;
  // 服务就绪时做一次 drain(覆盖"通知机制建立前 shell 已发送过"的窗口,幂等;含 shell 重启)
  // 1.9.0:就绪回调带 sid → 只 drain 该会话自己的实例
  setServiceReadyHandler((sid: string) => {
    void drainAnnotate(sid);
  });
  // 启动监听并把地址交给 shell(spawn 时 --notify-url /notify/<sid>,就绪后 /api/notify-url 注册)
  // 1.9.0:通知按 URL 里的 sid 归属 → 彻底取代旧的「最后活跃会话」投递
  startNotifyServer((sid: string) => {
    void drainAnnotate(sid);
  })
    .then((p) => log.info(`[notify] listening 127.0.0.1:${p}/notify/<sid> (event-driven; per-session routing)`))
    .catch((e) => log.error("[notify] listen failed (only initial/service-ready drain available)", e as Error));
}

/** 拉取某会话待发送的记录并投递给**该会话**(推送触发;空结果不记日志) */
async function drainAnnotate(sid: string): Promise<void> {
  if (!sid) return;
  if (drainingSid.has(sid)) return;
  const c = annotateClient;
  if (!c) return;
  // 该会话实例未启动/已关闭:跳过(服务就绪或下次通知到达时会再次触发;**绝不因 drain 而 spawn**)
  if (!isRunning(sid)) return;
  drainingSid.add(sid);
  try {
    const r = await service.annotateConsumeSent({ _sessionId: sid });
    const records = r?.records || [];
    if (records.length === 0) return; // 空结果不刷屏
    const imgInfo = records
      .filter((x: any) => x.type === "screenshot" && x.image)
      .map((x: any) => `#${x.index}:${x.image.length}B`);
    log.info(`[drain] consume-sent -> ${records.length} record(s) ids=[${records.map((x: any) => x.index).join(",")}] images=[${imgInfo.join(",")}]`);
    const lines = ["User annotated via panel:", ""];
    for (const item of records) {
      const label =
        item.type === "screenshot"
          ? `[#${item.index}] 截图${item.url ? ` @ ${item.url}` : ""}`
          : `[#${item.index}] ${item.selector || "(no selector)"}${item.url ? ` @ ${item.url}` : ""}`;
      lines.push(`- ${label}${item.note ? `: ${item.note}` : ""}`);
      // 截图:落盘保留并写入文本路径。与参考实现(opencode-playwright-tool)一致 ——
      // 图片**不经 session.prompt**(服务端对 image/* part 返回 400),而是走
      // 「工具结果附件」(bt_screenshot)或「读记录返回缓存路径」(read_record_content)。
      if (item.type === "screenshot" && item.image) {
        const savedPath = saveShotImage(log, sid, item.index, item.image);
        if (savedPath) lines.push(`  (image saved: ${savedPath})`);
      }
      lines.push("");
    }
    const text = lines.join("\n");
    // 投递目标 = 通知 URL 里的 sid(1.9.0 会话隔离:取代旧的 hook-cache last-writer-wins)
    log.info(`[drain] sid=${sid} prompt parts=text-only textLen=${text.length}`);
    const res = await c.session.prompt({ path: { id: sid }, body: { noReply: false, parts: [{ type: "text", text }] } });
    const err = res?.error;
    if (err) {
      // 不再静默:写日志 + 页面通知(SDK 返回 {error} 不抛,必须显式检查)
      log.error(`[drain] prompt FAILED sid=${sid}: ${JSON.stringify(err).slice(0, 300)}`, new Error("prompt error"));
      try {
        await service.notify({ message: `发送记录失败: ${JSON.stringify(err).slice(0, 180)}`, type: "err" }, sid);
      } catch {}
    } else {
      log.info(`[drain] prompt OK sid=${sid} delivered ${records.length} record(s)`);
    }
  } catch (e) {
      log.error("annotate drain error", e as Error);
    } finally {
      drainingSid.delete(sid);
    }
}

// Export default for npm loader
export default opencodeBrowserTool;
