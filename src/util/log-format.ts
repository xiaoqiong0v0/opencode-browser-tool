/**
 * console / network 日志流的输出格式化（纯函数，供 `bt_cli` 与单测复用）。
 *
 * 聚合由 shell 侧完成（console `group=burst|none`；network `group=burst|key|none`），
 * 这里只把结构化 item 渲染成模型可读的紧凑文本：不改写计数、不二次聚合。
 * 输出给模型的文本最后统一走 `sanitizeForModel`（绝对路径 → `<path>`）。
 */
import { tf } from "../i18n/index.js";
import { sanitizeForModel } from "./sanitize.js";

/** 模型可见日志文本上限（字符）：日志是多行正文，不能沿用默认 300 的短文案上限 */
export const MAX_LOG_TEXT_CHARS = 20000;

/** console 流条目（对应 `/api/console-logs` 的 `data.items`） */
export interface ConsoleLogItem {
  level?: string;
  msg?: string;
  count?: number;
  firstTs?: number;
  lastTs?: number;
  phase?: string;
  source?: string;
  stack?: string;
  tab?: number;
}

/** network 流条目（对应 `/api/network-logs` 的 `data.items`） */
export interface NetworkLogItem {
  method?: string;
  url?: string;
  status?: number | string;
  type?: string;
  initiator?: string;
  durationMs?: number;
  failure?: string;
  phase?: string;
  count?: number;
  firstTs?: number;
  lastTs?: number;
  tab?: number;
  /** 形状由服务端决定（对象/字符串）；仅在 `--include-headers` 时渲染 */
  headers?: unknown;
}

/** 读流端点的 data 形状（console / network 同构） */
export interface LogStreamData<T> {
  items?: T[];
  dropped?: number;
  bytes?: number;
  total?: number;
}

/** 聚合计数后缀：只有 count > 1 才显示，避免单条噪声 */
function countSuffix(count?: number): string {
  return typeof count === "number" && count > 1 ? ` (x${count})` : "";
}

/** 取 stack 首行（异常条目只展示首行，完整栈不进上下文） */
function firstLine(text: string): string {
  return String(text).split(/\r?\n/, 1)[0].trim();
}

/** 渲染单条 console 日志：`[level] msg (xN) phase=… source=…`，异常追加 stack 首行 */
export function formatConsoleItem(item: ConsoleLogItem): string {
  const level = item.level || "log";
  const msg = item.msg ?? "";
  const parts = [`[${level}] ${msg}${countSuffix(item.count)}`];
  if (item.phase) parts.push(`phase=${item.phase}`);
  if (item.source) parts.push(`source=${item.source}`);
  const line = parts.join(" ");
  const stack = item.stack ? firstLine(item.stack) : "";
  return stack ? `${line}\n  stack: ${stack}` : line;
}

/** 批量渲染 console 条目（空数组返回空数组，由调用方给「空结果」文案） */
export function formatConsoleItems(items?: ConsoleLogItem[]): string[] {
  return (items || []).map(formatConsoleItem);
}

/** 展示用 URL 上限（字符）：超出保留 host + 末尾 60 字符 */
const MAX_URL_CHARS = 120;

/** 超长 URL 保留的末尾字符数（中间用 `…` 省略） */
const URL_TAIL_CHARS = 60;

/** 省略标记 */
const URL_ELLIPSIS = "…";

/**
 * 判断 network 条目是否属于页面文档本身（用于推断同源基准）。
 * shell 侧契约里文档请求的 `type` 是 `doc`（Windows `document` 已归一为 `doc`）。
 */
function isDocumentType(type?: string): boolean {
  const t = (type || "").toLowerCase();
  return t === "doc" || t === "document";
}

/** 从同批条目推断页面 origin（取第一条文档请求）；推断不出返回 `undefined` */
function inferPageOrigin(items: NetworkLogItem[]): string | undefined {
  for (const it of items) {
    if (!isDocumentType(it.type)) continue;
    try {
      return new URL(String(it.url || "")).origin;
    } catch {
      // 该条 URL 不可解析：继续找下一条
    }
  }
  return undefined;
}

/**
 * 拼接并压到 `MAX_URL_CHARS` 内：超长时保留前缀（host）+ 末尾 `URL_TAIL_CHARS` 字符。
 * @param prefix 前缀（同源为 `host`，跨域/绝对为 `scheme//host`）
 * @param tail path + query
 * @returns 紧凑展示串
 */
function compactUrl(prefix: string, tail: string): string {
  const full = `${prefix}${tail}`;
  if (full.length <= MAX_URL_CHARS) return full;
  const keepFrom = Math.max(0, tail.length - URL_TAIL_CHARS);
  return `${prefix}/${URL_ELLIPSIS}${tail.slice(keepFrom)}`;
}

/**
 * URL → 紧凑展示：同源去 scheme 保留 `host + path + query`（`web.zhihjy.cn/js/app.js`），
 * 跨域保留完整 URL（`https://other.host/...`）；超长保留 host + 末尾 60 字符。
 *
 * **绝不以 `/` 开头输出裸路径**——`sanitizeForModel` 会把裸 POSIX 路径掩码成 `<path>`
 * （`/api/*`、`/notify/*` 例外），那是历史实测里 `[200] GET <path>` 的根因。
 *
 * @param raw 原始 URL（CDP 上报，通常为绝对 URL）
 * @param opts.origin 页面 origin（用于判定同源；缺省时退化为去 scheme 形式）
 * @returns 展示串；空输入返回空串
 */
export function displayUrl(raw?: string, opts?: { origin?: string }): string {
  const s = (raw || "").trim();
  if (!s) return "";
  let u: URL;
  try {
    u = new URL(s);
  } catch {
    // 相对/不可解析：加 `./` 前缀，既保留原信息又避免被当成绝对路径掩码
    return s.startsWith("/") ? `.${s}` : s;
  }
  if (!u.host) return s; // `data:` / `about:blank` 等无 host 的 URL 原样返回
  const tail = `${u.pathname}${u.search}`;
  const crossOrigin = Boolean(opts?.origin) && opts?.origin !== u.origin;
  return crossOrigin
    ? compactUrl(`${u.protocol}//${u.host}`, tail)
    : compactUrl(u.host, tail);
}

/** initiator 前缀类型（Windows CDP 会带 `script ` / `fetch ` 等前缀） */
const INITIATOR_TYPES = new Set([
  "document", "script", "stylesheet", "image", "font", "xhr", "fetch",
  "other", "subresource", "media", "manifest", "texttrack", "websocket",
  "parser", "preflight",
]);

/** 只显示类型、不显示 URL 的 initiator(解析器/预检/文档本身) */
const INITIATOR_TYPE_ONLY = new Set(["parser", "preflight", "document"]);

/** 无归因时的占位（调用方据此不输出 `from`） */
const INITIATOR_FALLBACK = "other";

/** URL scheme 前缀（`https://` / `ws://` …） */
const URL_SCHEME_RE = /^[a-z][a-z0-9+.-]*:\/\//i;

/** `file:line` / `file:line:col` 的行号后缀 */
const LINE_SUFFIX_RE = /^(.*?):(\d+)(?::\d+)?$/;

/**
 * initiator → 紧凑归因：去掉类型前缀与 scheme/host，只留**最后两段 + `:line`**
 * （如 `js/chunk-vendors.js:3498`）。
 *
 * 健壮性：空值 / `other` / 解析不出文件名 / 行号为 `0`（CDP 无行号时的占位，如 `BASE_DIGITAL:0`）
 * 统一返回 `other`；**绝不以 `/` 开头输出**（裸路径会被 `sanitizeForModel` 掩码）。
 * Linux 的 `subresource` / `document` 无栈，原样保留。
 *
 * @param raw 原始 initiator（`<type> <url>:<line>` 或平台标记）
 * @returns 归因串；无归因时为 `other`
 */
export function shortInitiator(raw?: string): string {
  let s = (raw || "").trim();
  if (!s) return INITIATOR_FALLBACK;
  const sp = s.indexOf(" ");
  const type = sp > 0 ? s.slice(0, sp).toLowerCase() : "";
  if (type && INITIATOR_TYPE_ONLY.has(type)) return type;
  if (sp > 0 && INITIATOR_TYPES.has(type)) s = s.slice(sp + 1).trim();
  if (!s || s.toLowerCase() === INITIATOR_FALLBACK) return INITIATOR_FALLBACK;
  // 去 scheme + host（首段是主机名，不参与「最后两段」）
  const hadHost = URL_SCHEME_RE.test(s);
  s = s.replace(URL_SCHEME_RE, "").replace(/^\/+/, "");
  s = s.split(/[?#]/, 1)[0] || "";
  const segs = s.split(/[\\/]/).filter(Boolean);
  if (hadHost && segs.length) segs.shift();
  if (!segs.length) return INITIATOR_FALLBACK;
  const m = LINE_SUFFIX_RE.exec(segs[segs.length - 1]);
  const line = m ? m[2] : "";
  const file = m ? m[1] : segs[segs.length - 1];
  if (!file) return INITIATOR_FALLBACK;
  if (line && Number(line) === 0) return INITIATOR_FALLBACK;
  const tail = [segs.length >= 2 ? segs[segs.length - 2] : "", file].filter(Boolean).join("/");
  return line ? `${tail}:${line}` : tail;
}

/**
 * 渲染 headers（形状按契约未定：对象 / 数组 / 字符串均容错）。
 * 默认已由服务端脱敏（Authorization/Cookie/Set-Cookie → `<redacted>`）；
 * `--reveal-secrets` 时展示原值，风险见 README。
 */
export function formatHeaders(headers: unknown): string {
  if (headers === undefined || headers === null) return "";
  if (typeof headers === "string") return headers.trim();
  if (Array.isArray(headers)) return headers.map((h) => String(h)).join("; ");
  if (typeof headers === "object") {
    return Object.entries(headers as Record<string, unknown>)
      .map(([k, v]) => `${k}: ${Array.isArray(v) ? v.join(", ") : String(v)}`)
      .join("; ");
  }
  return String(headers);
}

/** `from` 归因的展示值：`other` 视为无归因，不输出（避免每行都挂 `from other`） */
function initiatorLabel(raw?: string): string {
  const s = shortInitiator(raw);
  return s === INITIATOR_FALLBACK ? "" : s;
}

/**
 * 渲染单条 network 请求：`[status] METHOD host/path (xN) 30ms avg 失败: … from file:line phase=…`
 * @param item network 条目
 * @param opts.includeHeaders 是否附加 headers
 * @param opts.pageOrigin 页面 origin（判定同源用；缺省退化为去 scheme 形式）
 */
export function formatNetworkItem(
  item: NetworkLogItem,
  opts?: { includeHeaders?: boolean; pageOrigin?: string },
): string {
  const statusNum = typeof item.status === "number" ? item.status : Number(item.status);
  const status = Number.isFinite(statusNum) ? statusNum : 0;
  const badge = `[${status}]`.padEnd(5);
  const method = (item.method || "?").toUpperCase();
  const url = displayUrl(item.url, { origin: opts?.pageOrigin });
  const count = typeof item.count === "number" && item.count > 1 ? item.count : 0;
  // shell 聚合只保留同组**最后一条**的耗时（无 avg/min/max 字段），因此标注 `last`，
  // 不谎报平均值（契约示例里的 `avg (min-max)` 需要契约外字段，见交付说明）。
  const duration = typeof item.durationMs === "number" && Number.isFinite(item.durationMs)
    ? `${Math.round(item.durationMs)}ms${count ? " last" : ""}`
    : "";
  const failed = item.failure
    ? tf("cli.log.failed", { reason: item.failure })
    : status === 0 ? tf("cli.log.failed_unknown") : "";
  const from = initiatorLabel(item.initiator);
  const parts = [`${badge} ${method} ${url}${countSuffix(item.count)}`];
  if (duration) parts.push(duration);
  if (failed) parts.push(failed);
  if (from) parts.push(`from ${from}`);
  if (item.phase) parts.push(`phase=${item.phase}`);
  const line = parts.join(" ");
  const headers = opts?.includeHeaders ? formatHeaders(item.headers) : "";
  return headers ? `${line}\n  headers: ${headers}` : line;
}

/**
 * 批量渲染 network 条目；页面 origin 由同批的文档请求（`type=doc`）推断，
 * 用于把同源 URL 压成 `host/path`、跨域 URL 保留完整形式。
 */
export function formatNetworkItems(
  items?: NetworkLogItem[],
  opts?: { includeHeaders?: boolean; pageOrigin?: string },
): string[] {
  const list = items || [];
  const pageOrigin = opts?.pageOrigin ?? inferPageOrigin(list);
  return list.map((item) => formatNetworkItem(item, { includeHeaders: opts?.includeHeaders, pageOrigin }));
}

/** 缓冲逐出说明：有丢弃才追加，避免正常输出噪声 */
export function droppedNote(dropped?: number): string {
  return typeof dropped === "number" && dropped > 0 ? tf("cli.log.dropped", { n: dropped }) : "";
}

/**
 * 拼装最终文本：多行 + 可选「丢弃条目」说明，再统一脱敏（绝对路径 → `<path>`）。
 * 空结果返回空串，由命令层给「可能无日志/已导航/在其它标签」的明确文案。
 */
export function renderLogText(lines: string[], dropped?: number): string {
  const body = [lines.join("\n"), droppedNote(dropped)].filter(Boolean).join("\n");
  if (!body) return "";
  // `file:///C:/x` / `file:///home/x` 里的绝对路径 sanitizeForModel 不会命中(URL 保护 + 前导 `/`),
  // 在日志出口先整体掩码;http(s) URL 仍原样保留(模型需要看接口地址)。
  return sanitizeForModel(body.replace(/\bfile:\/\/[^\s"'<>]+/gi, "<path>"), MAX_LOG_TEXT_CHARS);
}
