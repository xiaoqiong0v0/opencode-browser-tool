/**
 * 下载列表 / 下载目录（1.13.0）的纯逻辑：目录模板展开、请求体构造、列表文本格式化。
 *
 * 不接触 fs / 网络 / i18n（文案由调用方传入 `DownloadsLabels`），便于单测。
 * 端点与字段口径见 `rust/src/downloads.rs`（`/api/downloads`、`/api/download-dir`）。
 */

/** 默认下载目录模板：插件侧展开后经 `--download-dir` 传给 shell */
export const DEFAULT_DOWNLOAD_DIR_TEMPLATE = "<cache>/sessions/<profile>/<sid>/downloads";
/** 每个目录保留的已完成记录数默认值（等价 env `BT_DOWNLOAD_HISTORY`） */
export const DEFAULT_DOWNLOAD_HISTORY = 64;
/** `<sname>` 路径段长度上限（字符） */
export const SNAME_MAX_LEN = 64;
/** 单条会话 id 路径段长度上限（字符；只做防护，正常 sid 远小于此值） */
const SID_MAX_LEN = 128;

/** 文件名不安全字符：Windows 非法字符 + 控制字符 */
const SEGMENT_FORBIDDEN = /[\\/:*?"<>|\u0000-\u001f\u007f]/g;
/** 空白折叠（多个空白压成一个空格） */
const WHITESPACE_RUN = /\s+/g;
/** 段尾的 `.` / 空格（Windows 会静默去掉，这里主动清理） */
const TRAILING_DOTS = /[. ]+$/g;

/** 下载状态（与 Rust `DownloadState` 的 serde 小写枚举一致） */
export type DownloadState = "progress" | "completed" | "failed" | "canceled";

/** 单条下载记录（`/api/downloads` 的 camelCase 字段） */
export interface DownloadItem {
  id: string;
  url: string;
  filename: string;
  path: string;
  totalBytes: number;
  receivedBytes: number;
  state: DownloadState;
  error?: string;
  startedAt: number;
}

/** 按存储目录分组的一组下载 */
export interface DownloadGroup {
  dir: string;
  items: DownloadItem[];
}

/** `/api/downloads` 的响应体 */
export interface DownloadsSnapshot {
  dir?: string;
  groups?: DownloadGroup[];
}

/** `/api/downloads {delete}` 的响应体（`deleted:false` = 记录已删但文件没删掉） */
export interface DownloadDeleteResult {
  deleted?: boolean;
  /** 被删记录的文件名（shell 找不到记录时不会有） */
  filename?: string;
  /** 文件没删掉的原因（`deleted:false` 时给出） */
  error?: string;
}

/** 输出文案（由插件层按 toolLang 注入，util 不依赖 i18n） */
export interface DownloadsLabels {
  /** 无任何记录时的整段文案 */
  empty: string;
  /** 总量未知时的标注（如“不确定”/“unknown”） */
  unknownSize: string;
  /** 状态名（按 `DownloadState` 取值） */
  states: Record<DownloadState, string>;
}

/** 模板占位符取值 */
export interface DownloadDirVars {
  /** `<cache>`：插件缓存根 */
  cacheDir: string;
  /** `<profile>`：当前 profile 名 */
  profile: string;
  /** `<sid>`：会话 id（spawn 时为归并后的浏览器 key） */
  sid: string;
  /** `<sname>`：会话标题（空则回退 `<sid>`） */
  sessionName: string;
}

/**
 * 清理路径段：去掉文件名非法字符与控制字符、折叠空白、去掉段尾 `.`/空格，并按上限截断。
 *
 * @param raw 原始片段（会话标题 / 会话 id）
 * @param maxLen 截断上限（字符）
 * @returns 可安全用作单级目录名的片段；全部被清空时返回空串
 */
function cleanSegment(raw: string, maxLen: number): string {
  const s = String(raw ?? "")
    .replace(WHITESPACE_RUN, " ")
    .replace(SEGMENT_FORBIDDEN, "")
    .replace(WHITESPACE_RUN, " ")
    .trim()
    .replace(TRAILING_DOTS, "");
  return s.slice(0, maxLen);
}

/**
 * 会话标题 → `<sname>` 路径段：去掉 `\ / : * ? " < > |` 与控制字符，折叠空白，
 * 段尾 `.`/空格清理，截断到 {@link SNAME_MAX_LEN} 字符。
 *
 * @param raw 会话标题（可为空）
 * @returns 安全化后的片段；全空返回空串（调用方回退 `<sid>`）
 */
export function sanitizeSessionName(raw: string): string {
  return cleanSegment(raw, SNAME_MAX_LEN);
}

/**
 * 展开下载目录模板。
 *
 * 支持占位符 `<cache>` / `<profile>` / `<sid>` / `<sname>`；`<sname>` 为空时回退 `<sid>`，
 * 两者都为空时回退 `default`（与 Rust 侧兜底一致）。返回值**原样拼接**、不 resolve，
 * 绝对/相对由配置决定（Rust 只接受绝对路径，非法值会被它拒绝并回退默认）。
 *
 * @param template 模板（可来自配置 `downloadDir` 或 env `BT_DOWNLOAD_DIR`）
 * @param vars 占位符取值
 * @returns 展开后的目录字符串
 */
export function expandDownloadDir(template: string, vars: DownloadDirVars): string {
  const sid = cleanSegment(vars.sid, SID_MAX_LEN);
  const sname = sanitizeSessionName(vars.sessionName) || sid || "default";
  return String(template ?? "")
    .split("<cache>").join(vars.cacheDir)
    .split("<profile>").join(cleanSegment(vars.profile, SNAME_MAX_LEN) || "default")
    .split("<sid>").join(sid || "default")
    .split("<sname>").join(sname);
}

/**
 * `/api/downloads` 请求体：只放实际给出的字段（未传的不下发，交给 shell 默认）。
 *
 * `delete` 与 `clear`/`cancel` 互斥语义由 shell 保证（`delete` 独占：只删这一条）。
 *
 * @param a 已解析命令参数（`limit` / `clear` / `cancel` / `delete`）
 * @returns 请求体对象
 */
export function buildDownloadsPayload(a: Record<string, any>): Record<string, any> {
  const p: Record<string, any> = {};
  if (a?.limit !== undefined && a?.limit !== "") p.limit = a.limit;
  if (a?.clear === true) p.clear = true;
  const cancel = typeof a?.cancel === "string" ? a.cancel.trim() : "";
  if (cancel) p.cancel = cancel;
  const del = typeof a?.delete === "string" ? a.delete.trim() : "";
  if (del) p.delete = del;
  return p;
}

/**
 * `/api/download-dir` 请求体：给了 `--set`（哪怕空串）就下发 `set`，交由 shell 校验并报错，
 * 不在插件侧静默降级成「查询」。
 *
 * @param a 已解析命令参数（`set`）
 * @returns 请求体对象
 */
export function buildDownloadDirPayload(a: Record<string, any>): Record<string, any> {
  if (typeof a?.set === "string") return { set: a.set };
  return {};
}

/**
 * `--limit` 合法性：只接受正整数；未给出返回 null。
 *
 * @param raw 解析后的 limit（parseArgs 已把数字 flag 转成 number）
 * @returns 非法时返回原因码 `invalid`，合法/未给出返回 null
 */
export function downloadsLimitIssue(raw: unknown): "invalid" | null {
  if (raw === undefined || raw === null || raw === "") return null;
  const n = typeof raw === "number" ? raw : Number(String(raw).trim());
  if (!Number.isInteger(n) || n <= 0) return "invalid";
  return null;
}

/**
 * 字节数 → 紧凑文本（`0B` / `640KB` / `1.2MB` / `12MB`）。
 *
 * @param bytes 字节数（非有限或 ≤0 均按 `0B`）
 * @returns 带单位的简短文本
 */
export function formatBytes(bytes: number): string {
  const units = ["B", "KB", "MB", "GB", "TB"];
  if (!Number.isFinite(bytes) || bytes <= 0) return "0B";
  let value = bytes;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  // 一位小数后去掉多余的 .0（3 MiB → "3MB"，1.25 MiB → "1.3MB"）
  const rounded = Math.round(value * 10) / 10;
  const text = value >= 10 || unit === 0 ? String(Math.round(value)) : String(rounded);
  return `${text}${units[unit]}`;
}

/**
 * 目录尾段（组标题用）：去掉尾随分隔符后取最后一段；根路径（如 `C:\`、`/`）原样回显。
 *
 * @param dir 目录路径
 * @returns 尾段名
 */
export function dirTail(dir: string): string {
  const raw = String(dir ?? "");
  const parts = raw.replace(/[\\/]+$/, "").split(/[\\/]/);
  return parts[parts.length - 1] || raw || "?";
}

/**
 * 来源主机：解析失败时返回空串（不显示原始 URL）。
 *
 * @param url 下载来源 URL
 * @returns host[:port]，解析失败为空串
 */
export function hostOf(url: string): string {
  try {
    return new URL(String(url)).host;
  } catch {
    return "";
  }
}

/**
 * 单条下载的行文本：`文件名  已收/总量  状态[ 百分比|不确定]  来源host[ — 错误]`。
 *
 * @param item 下载记录
 * @param labels 状态名 / 不确定标注文案
 * @returns 单行文本（不含缩进）
 */
export function formatDownloadRow(item: DownloadItem, labels: DownloadsLabels): string {
  const total = Number(item.totalBytes) || 0;
  const received = Number(item.receivedBytes) || 0;
  const size = total > 0 ? `${formatBytes(received)}/${formatBytes(total)}` : formatBytes(received);
  const state = labels.states[item.state] ?? String(item.state);
  // 进行中才给进度：总量已知给百分比，未知如实标「不确定」，不假装 0%
  const progress =
    item.state === "progress"
      ? ` ${total > 0 ? `${Math.min(100, Math.round((received / total) * 100))}%` : labels.unknownSize}`
      : "";
  const name = item.filename || item.path || item.id;
  const host = hostOf(item.url);
  const err = item.state === "failed" && item.error ? ` — ${item.error}` : "";
  return [name, size, `${state}${progress}`, host].filter((s) => s !== "").join("  ") + err;
}

/**
 * 整个列表的文本：按目录分组，组标题 `目录尾段 (n)`，组内每行缩进两格。
 *
 * @param data `/api/downloads` 响应体（可能为空/缺字段）
 * @param labels 文案
 * @returns 多行文本；无记录时返回 `labels.empty`
 */
export function formatDownloads(data: DownloadsSnapshot | undefined, labels: DownloadsLabels): string {
  const groups: DownloadGroup[] = data && Array.isArray(data.groups) ? data.groups : [];
  const lines: string[] = [];
  for (const group of groups) {
    const items = Array.isArray(group?.items) ? group.items : [];
    lines.push(`${dirTail(group?.dir ?? "")} (${items.length})`);
    for (const item of items) lines.push(`  ${formatDownloadRow(item, labels)}`);
  }
  return lines.length > 0 ? lines.join("\n") : labels.empty;
}
