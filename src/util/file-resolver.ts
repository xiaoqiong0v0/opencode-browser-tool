/**
 * `@file:` 两层解析的分工判定（纯逻辑，便于单测）。
 *
 * 背景：`bt_cli` 的通用层只允许**插件 tmp**内的 `@file:`（读完还会删掉全局 tmp 文件），
 * 而部分命令自带更宽的解析规则（工作区目录 + 插件 tmp，且不删文件）：
 * - `type --text`（见 `resolveTypeText`）
 * - `evaluate --script`（见 `runEvalCommand`）
 *
 * 若通用层先跑，工作区内的脚本会被直接拒掉（1.14.0 及更早的实际缺陷：项目内
 * `.tmp/xxx.js` 传 `evaluate --script @file:...` 报 file_outside_tmp）。
 * 故这些组合必须跳过通用层，交由命令自身判定。
 */

/** 自带文件解析的组合：命令名 → 需要放行的参数键。 */
const OWN_RESOLVER_KEYS: Readonly<Record<string, readonly string[]>> = Object.freeze({
  type: ["text"],
  evaluate: ["script"],
});

/**
 * 判断某个命令的某个参数是否走命令自身的 `@file:` 解析（应跳过仅插件 tmp 的通用层）。
 *
 * @param cmd 命令名（如 `evaluate` / `type`）
 * @param key 参数键（如 `script` / `text`）
 * @returns 走自身解析返回 true；否则 false（由通用层处理）
 */
export function usesOwnFileResolver(cmd: string, key: string): boolean {
  const keys = OWN_RESOLVER_KEYS[cmd];
  return Array.isArray(keys) && keys.includes(key);
}
