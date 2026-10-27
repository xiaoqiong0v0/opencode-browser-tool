/**
 * `inject`（持久注入规则）的纯逻辑：item 构造 / 上限判定 / 输出格式化。
 *
 * 文件读取与 containment 在 `index.ts`（复用 `evaluate` 的 `readEvalFile`），HTTP 在
 * `client.ts`（`/api/inject/rules`、`/api/inject/test`）；这里只做可单测的纯函数，
 * 不含任何 IO，便于 `tests/unit/inject-rules.test.mjs` 直接调用。
 *
 * 字段全部以契约为准（`.tmp/scratch/inject-rules-contract.md`），不臆造 Rust 侧字段：
 * 规则 = `{ id, match, enabled, order, items, last, failStreak }`，
 * item 目前只有 `{ kind:"text", code, name }` 一种（文件内容在插件侧转成 text）。
 */
import { basename } from "path";
import { sanitizeForModel } from "./sanitize.js";

/** 单条规则 items 合计字节上限（256 KiB，与契约同口径） */
export const INJECT_RULE_MAX_BYTES = 256 * 1024;

/** 规则条数上限（契约：≤ 20） */
export const INJECT_MAX_RULES = 20;

/** `failStreak` 达到该值后 Rust 自动禁用该规则（契约：>= 3） */
export const INJECT_FAIL_STREAK_LIMIT = 3;

/** `list` 时每条规则最多展开的 item 明细条数（超出只给数量） */
export const INJECT_LIST_MAX_ITEMS = 5;

/** `last.error` 展示上限（字符；进表格前先脱敏，避免回显执行栈里的本机路径） */
export const INJECT_ERROR_MAX_CHARS = 120;

/** 内联代码项的统一 name（没有文件名可用） */
const TEXT_ITEM_NAME = "text";

/** 字节展示换算（KiB） */
const BYTES_PER_KIB = 1024;

/** 字节展示保留位数 */
const KIB_DIGITS = 1;

/** 换行匹配（错误信息只取首行） */
const LINE_BREAK_RE = /\r?\n/;

/** 一条注入项：契约当前只有文本类（文件内容由插件侧读入后转成 text） */
export interface InjectItem {
  kind: "text";
  code: string;
  name: string;
}

/** 规则最近一次执行结果（Rust 写回；字段可缺省，展示时按缺省处理） */
export interface InjectRuleLast {
  ts?: number;
  ok?: number;
  failed?: number;
  error?: string;
}

/** 一条持久注入规则（服务端返回的形态） */
export interface InjectRule {
  id: string;
  match: string;
  enabled?: boolean;
  order?: number;
  items?: InjectItem[];
  last?: InjectRuleLast;
  failStreak?: number;
}

/** `/api/inject/rules` 的 `data`（add 额外带 `rule`） */
export interface InjectRulesData {
  rules?: InjectRule[];
  rule?: InjectRule;
}

/** `/api/inject/test` 的 `data`（只算不执行） */
export interface InjectTestData {
  matched?: string[];
  items?: number;
}

/** 待转成 item 的文件项（`code` 已由调用方读入） */
export interface InjectFileSource {
  /** 模型传入的路径（只用于取 basename 作 name，不发路径给 Rust） */
  path: string;
  /** 文件内容 */
  code: string;
}

/**
 * 把参数分类成有序 item 列表：先 `--file`（按传入顺序，name=basename），后 `--text`
 * （按传入顺序，name 固定 `text`）。Rust 侧不接触路径，只收到 `{kind,code,name}`。
 *
 * @param files 已读入的文件项
 * @param texts 内联代码文本
 * @returns 有序 item 列表（可能为空，由调用方判空报错）
 */
export function buildInjectItems(files: InjectFileSource[], texts: string[]): InjectItem[] {
  const items: InjectItem[] = files.map((f) => ({
    kind: "text",
    code: f.code,
    name: basename(f.path) || f.path,
  }));
  for (const code of texts) items.push({ kind: "text", code, name: TEXT_ITEM_NAME });
  return items;
}

/**
 * 单个 item 的 UTF-8 字节数（体积口径与 fs 一致，不用字符数）。
 *
 * @param item 注入项
 * @returns 字节数
 */
export function injectItemBytes(item: InjectItem): number {
  return Buffer.byteLength(item.code ?? "", "utf8");
}

/**
 * items 合计字节数。
 *
 * @param items 注入项列表
 * @returns 合计 UTF-8 字节数
 */
export function injectItemsBytes(items: InjectItem[]): number {
  return items.reduce((sum, item) => sum + injectItemBytes(item), 0);
}

/**
 * 单条规则 items 是否超上限（超过 `INJECT_RULE_MAX_BYTES` 即拒绝，不带坏规则进浏览器）。
 *
 * @param items 注入项列表
 * @returns 超限时返回上限 KiB（供文案使用），未超限返回 null
 */
export function injectRuleLimitIssue(items: InjectItem[]): { kb: number } | null {
  return injectItemsBytes(items) > INJECT_RULE_MAX_BYTES ? { kb: INJECT_RULE_MAX_BYTES / BYTES_PER_KIB } : null;
}

/**
 * `add` 子动作的参数校验（纯判定，不读文件）。
 *
 * @param match 已 trim 的 `--match` 值
 * @param fileCount `--file` 个数
 * @param textCount `--text` 个数
 * @returns 原因码：`no_match` / `no_items`；通过返回 null
 */
export function injectAddIssue(match: string, fileCount: number, textCount: number): "no_match" | "no_items" | null {
  if (!match) return "no_match";
  if (fileCount + textCount === 0) return "no_items";
  return null;
}

/**
 * 规范化子动作名：未给出时按只读的 `list` 处理；未知名字原样返回，由调用方报错。
 *
 * @param raw 第一个位置参数
 * @returns 子动作名
 */
export function normalizeInjectAction(raw: string | undefined): string {
  const action = (raw || "").trim().toLowerCase();
  return action || "list";
}

/**
 * 字节数的人类可读形式（`512B` / `1.2KiB`）。
 *
 * @param bytes 字节数
 * @returns 展示文本
 */
export function formatInjectBytes(bytes: number): string {
  const n = Number.isFinite(bytes) && bytes > 0 ? bytes : 0;
  if (n < BYTES_PER_KIB) return `${n}B`;
  return `${(n / BYTES_PER_KIB).toFixed(KIB_DIGITS)}KiB`;
}

/**
 * `list` 输出：表头 + 每规则一行（id / order / enabled / match / items / last），
 * 另附每条规则的 item 明细（**只给 name + 字节数，绝不输出脚本内容**）。
 *
 * 列名与状态文案固定英文（与 `formatEvalReport` 同约定：不随 toolLang 变化，保证结构稳定）。
 *
 * @param data 服务端 `data`（`rules` 可缺省）
 * @returns 多行文本（空列表时只有汇总行，调用方另行给空文案）
 */
export function formatInjectList(data: InjectRulesData | undefined): string {
  const rules = data?.rules ?? [];
  const header = `inject: ${rules.length} rule(s), limit ${INJECT_MAX_RULES}`;
  if (rules.length === 0) return header;

  const columns = ["id", "order", "enabled", "match", "items", "last"];
  const rows = rules.map((rule) => [
    String(rule.id ?? "-"),
    rule.order === undefined ? "-" : String(rule.order),
    rule.enabled === false ? "off" : "on",
    String(rule.match ?? ""),
    String((rule.items ?? []).length),
    lastCell(rule),
  ]);
  const widths = columns.map((c, i) => rows.reduce((w, row) => Math.max(w, row[i].length), c.length));
  /** 末列不补齐，避免行尾空白 */
  const layout = (cells: string[]): string =>
    cells.map((cell, i) => (i === cells.length - 1 ? cell : cell.padEnd(widths[i]))).join("  ");

  const lines = [header, layout(columns), ...rows.map(layout)];
  for (const [i, rule] of rules.entries()) {
    const detail = itemDetail(rule.items ?? []);
    if (detail) lines.push(`  ${rows[i][0]}: ${detail}`);
  }
  return lines.join("\n");
}

/**
 * `add` 输出：id + match + order + enabled + items 数（不含脚本内容）。
 *
 * @param rule 服务端返回的新建规则（缺省时退化为通用文案）
 * @returns 单行文本
 */
export function formatInjectAdd(rule: InjectRule | undefined): string {
  if (!rule || !rule.id) return "inject: added";
  const items = rule.items ?? [];
  const enabled = rule.enabled === false ? "false" : "true";
  const order = rule.order === undefined ? "-" : String(rule.order);
  return `inject: added ${rule.id}  match=${rule.match ?? ""}  order=${order}  enabled=${enabled}  items=${items.length}`;
}

/**
 * `test` 输出：命中的规则 id 与 item 总数（**只算不执行**）。
 *
 * @param data 服务端 `data`
 * @returns 单行文本
 */
export function formatInjectTest(data: InjectTestData | undefined): string {
  const matched = data?.matched ?? [];
  const items = data?.items ?? 0;
  if (matched.length === 0) return "inject test: no rule matched (not executed)";
  return `inject test: ${matched.length} rule(s) matched [${matched.join(", ")}], ${items} item(s) (not executed)`;
}

/**
 * 最近一次执行结果的展示：`ok/failed` + 错误首行；重试达到自动禁用阈值时一并标注。
 *
 * @param rule 规则
 * @returns 单行文本（无执行记录时为 `-`）
 */
function lastCell(rule: InjectRule): string {
  const parts: string[] = [];
  const last = rule.last;
  const hasLast =
    last !== undefined &&
    (last.ts !== undefined || last.ok !== undefined || last.failed !== undefined || !!last.error);
  if (!hasLast) {
    parts.push("-");
  } else {
    parts.push(`${last.ok ?? 0} ok / ${last.failed ?? 0} failed`);
    const err = firstLine(last.error);
    if (err) parts.push(`error: ${sanitizeForModel(err, INJECT_ERROR_MAX_CHARS)}`);
  }
  const streak = rule.failStreak ?? 0;
  if (streak >= INJECT_FAIL_STREAK_LIMIT) parts.push(`failStreak=${streak}`);
  return parts.join("  ");
}

/**
 * item 明细：`name + 字节数`，超出展示上限只给 `+N more`。
 *
 * @param items 注入项列表
 * @returns 单行明细（无 item 时返回空串）
 */
function itemDetail(items: InjectItem[]): string {
  if (items.length === 0) return "";
  const shown = items
    .slice(0, INJECT_LIST_MAX_ITEMS)
    .map((item) => `${item.name} ${formatInjectBytes(injectItemBytes(item))}`);
  const rest = items.length - INJECT_LIST_MAX_ITEMS;
  if (rest > 0) shown.push(`+${rest} more`);
  return shown.join(", ");
}

/**
 * 错误信息只保留首行（避免把多行堆栈塞进表格）。
 *
 * @param error 原始错误文本（可能缺省）
 * @returns 首行（无内容返回空串）
 */
function firstLine(error: string | undefined): string {
  if (!error) return "";
  return error.split(LINE_BREAK_RE)[0].trim();
}
