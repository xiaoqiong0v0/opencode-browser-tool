/**
 * `evaluate` 多客户端项的逻辑（分类 / 标签 / 串行执行编排 / 结果截断 / 报告格式化）。
 *
 * 文件读取、路径 containment、脚本执行的真实实现都在 `index.ts`（需要 fs 与浏览器服务）；
 * 这里通过 `EvalRunDeps` 注入，本模块自身不含任何 IO，便于 `tests/unit/eval-items.test.mjs` 直接调用。
 */
import { sanitizeForModel } from "./sanitize.js";

/** 文件项前缀：`@file:<path>` = 读该文件内容作为脚本 */
export const EVAL_FILE_PREFIX = "@file:";

/** 单条结果（或失败信息）展示上限（字符） */
export const EVAL_RESULT_MAX_CHARS = 500;

/** 单个脚本文件字节上限（256 KiB） */
export const EVAL_ITEM_MAX_BYTES = 256 * 1024;

/** 一次调用内所有脚本文件合计字节上限（1 MiB） */
export const EVAL_TOTAL_MAX_BYTES = 1024 * 1024;

/** 结果列的状态文案（模型可读、简短，不随 toolLang 变化，保持报告结构稳定） */
const STATUS_OK = "ok";
const STATUS_FAILED = "FAILED";

/** 报告首行前缀 */
const REPORT_PREFIX = "evaluate";

/** 项类型展示文案 */
const KIND_LABEL_FILE = "file";
const KIND_LABEL_TEXT = "text";

/** 截断与换行折叠标记 */
const ELLIPSIS = "…";
const LINE_BREAK_RE = /\r?\n/g;
const FLAT_LINE_BREAK = "\\n";

/** 单条 evaluate 项：文件脚本或内联文本（顺序即执行顺序） */
export type EvalItem = { kind: "file"; path: string } | { kind: "text"; text: string };

/** 分类结果：成功给出有序项列表；失败给出可映射到 i18n 的原因码 */
export type EvalItemsParse =
  | { ok: true; items: EvalItem[] }
  | { ok: false; reason: "script_conflict" | "no_items" };

/** 一条已执行项的结果（`output` 未经脱敏/截断，格式化时统一处理） */
export interface EvalResultEntry {
  label: string;
  ok: boolean;
  ms: number;
  output: string;
}

/** 报告汇总选项 */
export interface EvalReportOptions {
  /** 计划执行的总项数（停止时可能大于实际执行数） */
  total: number;
  /** 因某项失败而提前停止 */
  stopped: boolean;
  /** 带 `--continue-on-error` 执行（失败不中断；无失败时也会标注） */
  continued: boolean;
}

/**
 * 把位置参数分类为有序的 `evaluate` 项。
 *
 * 规则：以 `@file:` 开头的参数 = 文件项，其余 = 内联 JS 文本项。
 * `--script` 只允许单独使用：与位置参数同时给出时无法确定顺序，直接报冲突。
 *
 * @param positionals 位置参数（已由 string-argv 分词）
 * @param hasScriptFlag 是否传了 `--script`
 * @returns 成功时给出有序项列表；失败时给出原因码（`script_conflict` / `no_items`）
 */
export function classifyEvalArgs(positionals: string[], hasScriptFlag: boolean): EvalItemsParse {
  if (hasScriptFlag && positionals.length > 0) {
    return { ok: false, reason: "script_conflict" };
  }
  if (positionals.length === 0) {
    return { ok: false, reason: "no_items" };
  }
  const items: EvalItem[] = positionals.map((p) =>
    p.startsWith(EVAL_FILE_PREFIX)
      ? { kind: "file", path: p.slice(EVAL_FILE_PREFIX.length) }
      : { kind: "text", text: p },
  );
  return { ok: true, items };
}

/**
 * 项在报告里的标签（`file <path>` / `text`）；绝对路径由调用方按需脱敏。
 *
 * @param item 待展示的项
 * @returns 单行标签文本
 */
export function evalItemLabel(item: EvalItem): string {
  return item.kind === "file" ? `${KIND_LABEL_FILE} ${item.path}` : KIND_LABEL_TEXT;
}

/**
 * 把服务端返回的任意 JSON 值渲染成结果文本。
 *
 * 字符串结果原样输出（便于直接看页面取回的文本），其它值 `JSON.stringify`；
 * 循环引用等序列化失败回退 `String()`，绝不抛错。
 *
 * @param value `/api/evaluate` 的返回值
 * @returns 未截断的结果文本
 */
export function formatEvalValue(value: unknown): string {
  if (value === undefined) return "undefined";
  if (typeof value === "string") return value;
  try {
    const text = JSON.stringify(value);
    return text === undefined ? String(value) : text;
  } catch {
    return String(value);
  }
}

/**
 * 把多行文本折叠成单行（报告每条一行，便于模型按行读取）。
 *
 * @param text 原始文本
 * @returns 换行替换为字面量 `\n` 的单行文本
 */
export function flattenEvalText(text: string): string {
  return text.replace(LINE_BREAK_RE, FLAT_LINE_BREAK);
}

/**
 * 截断文本到指定长度（含省略号）。
 *
 * @param text 原始文本
 * @param max 上限（字符数，默认 500）
 * @returns 不超过 `max` 个字符的文本
 */
export function truncateEvalText(text: string, max: number = EVAL_RESULT_MAX_CHARS): string {
  const limit = Number.isFinite(max) && max > 0 ? Math.floor(max) : EVAL_RESULT_MAX_CHARS;
  if (text.length <= limit) return text;
  return `${text.slice(0, Math.max(0, limit - 1))}${ELLIPSIS}`;
}

/**
 * 渲染多客户端报告：首行汇总 + 每项一行（`[n] <label>  <status>  <ms>ms  → <body>`）。
 *
 * 失败项的结果经 `sanitizeForModel` 脱敏（绝对路径 → `<path>`）；所有结果统一折叠换行并截断到
 * `EVAL_RESULT_MAX_CHARS`，保证每条占用一行且长度可控。
 *
 * @param entries 已执行项（顺序即执行顺序；提前停止时可能少于 `total`）
 * @param options 汇总信息（总数 / 是否停止 / 是否 continue-on-error）
 * @returns 多行报告文本
 */
export function formatEvalReport(entries: EvalResultEntry[], options: EvalReportOptions): string {
  const okCount = entries.filter((e) => e.ok).length;
  const failedCount = entries.length - okCount;
  const suffix = options.stopped ? " (stop)" : options.continued ? " (continued)" : "";
  const header =
    `${REPORT_PREFIX}: ${options.total} item(s), ${okCount} ${STATUS_OK}` +
    `${failedCount > 0 ? `, ${failedCount} failed` : ""}${suffix}`;

  const labelWidth = entries.reduce((w, e) => Math.max(w, e.label.length), 0);
  const rows = entries.map((e, i) => {
    const label = e.label.padEnd(labelWidth);
    const status = e.ok ? STATUS_OK : STATUS_FAILED;
    return `[${i + 1}] ${label}  ${status}  ${e.ms}ms  → ${entryBody(e)}`;
  });
  return [header, ...rows].join("\n");
}

/** 文件项读取结果：成功给出脚本文本与字节数；失败给出可直接回模型的文案 */
export type EvalFileRead = { content: string; bytes: number } | { error: string };

/** 多客户端项执行所需的依赖（IO 与浏览器调用由调用方注入，便于单测替换） */
export interface EvalRunDeps {
  /** 读一个文件项；containment / 体积校验由调用方负责，失败时返回 `{ error }` */
  readFile: (path: string) => EvalFileRead;
  /** 执行单条脚本并返回服务端结果 */
  evaluate: (script: string) => Promise<unknown>;
  /** `--continue-on-error`：某条失败后是否继续执行后续项 */
  continued: boolean;
}

/**
 * 按顺序**串行**执行多客户端项，返回可直接回模型的报告。
 *
 * - 文件项在**任何脚本执行前**统一读入：任一路径越界 / 读取失败 / 超限 → 直接返回错误文案，
 *   **整次不执行任何脚本**（避免只跑了一半的副作用）；脚本执行失败只影响该项。
 * - 默认遇错即停；`deps.continued` 为 true（`--continue-on-error`）时继续执行后续项。
 * - 输出由 `formatEvalReport` 渲染（失败信息脱敏、每条截断到 `EVAL_RESULT_MAX_CHARS`）。
 *
 * @param items 有序执行项
 * @param deps 读文件 / 执行脚本 / 是否继续的依赖
 * @returns 报告文本（不含 timeout 提示，由调用方追加）
 */
export async function runEvalItems(items: EvalItem[], deps: EvalRunDeps): Promise<string> {
  const resolved: Array<{ item: EvalItem; script: string }> = [];
  for (const item of items) {
    if (item.kind === "text") {
      resolved.push({ item, script: item.text });
      continue;
    }
    const read = deps.readFile(item.path);
    if ("error" in read) return read.error;
    resolved.push({ item, script: read.content });
  }

  const entries: EvalResultEntry[] = [];
  let stopped = false;
  for (const { item, script } of resolved) {
    const startedAt = Date.now();
    const entry: EvalResultEntry = { label: sanitizeForModel(evalItemLabel(item)), ok: true, ms: 0, output: "" };
    try {
      entry.output = formatEvalValue(await deps.evaluate(script));
    } catch (e: any) {
      entry.ok = false;
      entry.output = e?.message || String(e);
      stopped = !deps.continued;
    }
    entry.ms = Date.now() - startedAt;
    entries.push(entry);
    if (stopped) break;
  }
  return formatEvalReport(entries, { total: resolved.length, stopped, continued: deps.continued });
}

/**
 * 单条结果体：折叠换行 → 失败项脱敏 → 截断。
 *
 * @param entry 已执行项
 * @returns 单行结果文本（≤ `EVAL_RESULT_MAX_CHARS`）
 */
function entryBody(entry: EvalResultEntry): string {
  const flat = flattenEvalText(entry.output);
  const safe = entry.ok ? flat : sanitizeForModel(flat, EVAL_RESULT_MAX_CHARS);
  return truncateEvalText(safe, EVAL_RESULT_MAX_CHARS);
}
