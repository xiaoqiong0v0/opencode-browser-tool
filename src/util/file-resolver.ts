/**
 * `@file:` 通用层的解析模式判定（纯逻辑，便于单测）。
 *
 * 背景：`bt_cli` 的通用层对所有字符串参数先做一遍 `@file:` 处理。它原本只允许
 * **插件 tmp**内的文件（读完还会删掉全局 tmp 里的文件），这对大多数命令是对的；
 * 但有两类命令自带更宽的语义，需要区别对待：
 *
 * - `type --text`：命令自身解析（`resolveTypeText`），通用层必须整体跳过；
 * - `evaluate --script`：脚本文件应当允许放在**工作区**（用户最常见用法），
 *   但语义仍是"读出内容当脚本"，所以不能跳过，要按 workspace 模式读且**不删文件**。
 *
 * 1.14.0 及更早：`evaluate --script @file:<工作区>/x.js` 会被 tmp-only 规则直接拒掉
 * （报 file_outside_tmp），工作区里的脚本永远跑不起来。
 */

/** 通用层处理某个参数时的模式。 */
export type FileResolutionMode =
  /** 命令自己解析该参数，通用层不介入。 */
  | "own"
  /** 允许 工作区目录 + 插件 tmp，读内容但不删文件。 */
  | "workspace"
  /** 只允许插件 tmp（默认；读完会删掉全局 tmp 里的临时文件）。 */
  | "tmp";

/** 命令名 → 参数键 → 模式（未列出的一律 `tmp`）。 */
const MODES: Readonly<Record<string, Readonly<Record<string, FileResolutionMode>>>> = Object.freeze({
  type: { text: "own" },
  evaluate: { script: "workspace" },
});

/**
 * 判断通用层该如何处理某命令的某个 `@file:` 参数。
 *
 * @param cmd 命令名（如 `evaluate` / `type`）
 * @param key 参数键（如 `script` / `text`）
 * @returns 解析模式（`own` / `workspace` / `tmp`）
 */
export function fileResolutionMode(cmd: string, key: string): FileResolutionMode {
  const perCmd = MODES[cmd];
  const mode = perCmd ? perCmd[key] : undefined;
  return mode ?? "tmp";
}
