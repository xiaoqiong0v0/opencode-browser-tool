/**
 * `type`（向当前焦点插入文本，不清空）的纯逻辑：文本校验、`@file:` 解析、输出格式化。
 *
 * 文件读取与 containment 由调用方注入（`index.ts` 复用 `evaluate` 的 `readEvalFile`），
 * 这里不含任何 IO，便于 `tests/unit/type-text.test.mjs` 直接调用 dist 产物。
 * 上限与 Rust 侧 `keyboard.rs` 的 `TYPE_TEXT_MAX_BYTES` 同口径（8 KiB，按字节计）。
 */
import { tf } from "../i18n/index.js";

/** 单次插入文本上限（字节；0 长度之外的入参超过即拒绝） */
export const TYPE_TEXT_MAX_BYTES = 8 * 1024;

/** `--text` 的文件引用前缀（与 `evaluate` 的 `@file:` 同形） */
export const TYPE_FILE_PREFIX = "@file:";

/** 文本身份校验结果：`null` 表示通过 */
export type TypeTextIssue = "required" | "too_large" | null;

/** 文件项读取结果（结构对齐 `index.ts` 的 `FileItemRead`） */
export type TypeTextRead = { content: string } | { error: string };

/**
 * 校验待插入文本。
 *
 * @param text 实际要插入的文本（`@file:` 已解析后的内容）
 * @returns `required`=空文本；`too_large`=超过 8 KiB；`null`=通过
 */
export function typeTextIssue(text: string): TypeTextIssue {
  if (text.length === 0) return "required";
  if (Buffer.byteLength(text, "utf-8") > TYPE_TEXT_MAX_BYTES) return "too_large";
  return null;
}

/**
 * 解析 `--text`：`@file:<path>` 交给注入的读取器（与 `evaluate` 同一套 containment），
 * 其余一律视为内联文本。
 *
 * @param raw 模型传入的原始 `--text` 值
 * @param readFile 文件读取器（越界 / 超限 / 读失败都返回 `{error}`，由调用方复用 `readEvalFile`）
 * @returns 插入文本，或可直接回模型的错误文案
 */
export function resolveTypeText(
  raw: string,
  readFile: (path: string) => TypeTextRead,
): { text: string } | { error: string } {
  if (!raw.startsWith(TYPE_FILE_PREFIX)) return { text: raw };
  const path = raw.slice(TYPE_FILE_PREFIX.length).trim();
  if (!path) return { error: tf("cli.err.type_text_required") };
  const read = readFile(path);
  if ("error" in read) return { error: read.error };
  return { text: read.content };
}

/**
 * 渲染 `type` 的成功输出。
 *
 * @param inserted 服务端回报的插入字符数
 * @param selector 本次显式聚焦的选择器（空串表示沿用当前焦点）
 * @param entered 是否补发了 Enter
 * @returns `typed N char(s) into <selector|focused element>[ + Enter]`
 */
export function formatTypeOutput(inserted: number, selector: string, entered: boolean): string {
  const target = selector || tf("cli.out.type.focused");
  const suffix = entered ? tf("cli.out.type.enter") : "";
  return tf("cli.out.type", { n: inserted, target }) + suffix;
}
