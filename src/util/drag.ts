/**
 * `drag` 命令参数的插件侧最小校验。
 *
 * 真正的解析/执行在 Rust `/api/drag`（服务端仍是最终口径）；这里只做互斥检查，
 * 让模型在插件侧就能拿到本地化错误，而不是先发一次 HTTP。
 */

/** 参数问题原因（与 i18n 键 `cli.err.drag_<reason>` 一一对应） */
export type DragArgIssue =
  | "actions_conflict"
  | "source_required"
  | "target_required"
  | "source_conflict"
  | "target_conflict";

/** 字段是否为非空字符串（空白视为未提供） */
function present(a: Record<string, unknown>, key: string): boolean {
  const v = a[key];
  return typeof v === "string" && v.trim() !== "";
}

/**
 * 校验 drag 参数组合。
 *
 * 规则：
 * - `actions` 与 selection/point 四参互斥；
 * - 非批次时 source 侧、target 侧各自必须且只能给一种（选择器或坐标）。
 *
 * @param a 已解析的命令参数
 * @returns 问题原因；合法返回 null
 */
export function dragArgIssue(a: Record<string, unknown>): DragArgIssue | null {
  // 与 Rust 同口径:只要出现 actions 键就按批次处理(非法类型交给服务端报错)
  const actions = a.actions !== undefined;
  const sides = ["sourceSelector", "sourcePoint", "targetSelector", "targetPoint"];
  if (actions) {
    return sides.some((k) => present(a, k)) ? "actions_conflict" : null;
  }
  const sourceCount = ["sourceSelector", "sourcePoint"].filter((k) => present(a, k)).length;
  if (sourceCount === 0) return "source_required";
  if (sourceCount > 1) return "source_conflict";
  const targetCount = ["targetSelector", "targetPoint"].filter((k) => present(a, k)).length;
  if (targetCount === 0) return "target_required";
  if (targetCount > 1) return "target_conflict";
  return null;
}
