/**
 * 触摸命令参数校验（插件侧，Rust 端仍为最终口径）。
 *
 * 只做结构/互斥校验，避免明显非法参数先发一次 HTTP；selector 的真实 CSS 语法由页面解析。
 */

/** 指针类型 */
export type TouchPointer = "mouse" | "touch";

/** 解析 `--pointer`（缺省由命令决定） */
export function parsePointer(
  v: unknown,
  fallback: TouchPointer,
): { ok: true; pointer: TouchPointer } | { ok: false } {
  if (v === undefined || v === null) return { ok: true, pointer: fallback };
  if (typeof v === "string") {
    const s = v.trim().toLowerCase();
    if (s === "") return { ok: true, pointer: fallback };
    if (s === "mouse" || s === "touch") return { ok: true, pointer: s };
  }
  return { ok: false };
}

/** 解析 `"x,y"` */
export function parsePointText(
  v: unknown,
): { ok: true; x: number; y: number } | { ok: false } {
  if (typeof v !== "string") return { ok: false };
  const parts = v.split(",");
  if (parts.length !== 2) return { ok: false };
  const x = Number(parts[0].trim());
  const y = Number(parts[1].trim());
  if (!Number.isFinite(x) || !Number.isFinite(y)) return { ok: false };
  return { ok: true, x, y };
}

/** touch actions 结构问题 */
export type TouchActionsIssue = "json" | "not_array" | "empty" | "missing_up" | "shape";

/** 校验 `touch --actions` 的 JSON（不解析 selector CSS） */
export function parseTouchActions(
  text: unknown,
): { ok: true; actions: unknown[] } | { ok: false; reason: TouchActionsIssue } {
  if (typeof text !== "string") return { ok: false, reason: "json" };
  let value: unknown;
  try {
    value = JSON.parse(text);
  } catch {
    return { ok: false, reason: "json" };
  }
  if (!Array.isArray(value)) return { ok: false, reason: "not_array" };
  if (value.length === 0) return { ok: false, reason: "empty" };
  let active = 0;
  for (const item of value) {
    if (!isTouchAction(item)) return { ok: false, reason: "shape" };
    const a = item as Record<string, any>;
    if ("down" in a) {
      if (active > 0) return { ok: false, reason: "shape" };
      active = a.down.length;
    } else if ("move" in a) {
      if (active === 0 || a.move.length !== active) return { ok: false, reason: "shape" };
    } else if ("up" in a) {
      if (active === 0) return { ok: false, reason: "shape" };
      active = 0;
    }
  }
  // 批次必须以 up 收尾,避免跨命令遗留按下
  const last = value[value.length - 1] as Record<string, any>;
  if (!isObj(last) || !("up" in last)) return { ok: false, reason: "missing_up" };
  return { ok: true, actions: value };
}

/** tap 参数问题 */
export type TapIssue = "selector_conflict" | "target_required" | "xy_pair" | "xy_invalid" | "pointer";

/** 校验 tap 目标/pointer */
export function tapIssue(a: Record<string, unknown>): TapIssue | null {
  if (!parsePointer(a.pointer, "touch").ok) return "pointer";
  const hasSel = typeof a.selector === "string" && a.selector.trim() !== "";
  const hasX = present(a.x);
  const hasY = present(a.y);
  if (hasSel && (hasX || hasY)) return "selector_conflict";
  if (!hasSel && !hasX && !hasY) return "target_required";
  if (hasX !== hasY) return "xy_pair";
  if (hasX && hasY) {
    const x = Number(a.x);
    const y = Number(a.y);
    if (!Number.isFinite(x) || !Number.isFinite(y)) return "xy_invalid";
  }
  return null;
}

/** 非空值(字符串 trim 后为空视为未提供) */
function present(v: unknown): boolean {
  return v !== undefined && v !== null && !(typeof v === "string" && v.trim() === "");
}

/** swipe 参数问题 */
export type SwipeIssue = "from_invalid" | "to_invalid" | "steps_invalid" | "duration_invalid";

/** 校验 swipe 参数 */
export function swipeIssue(a: Record<string, unknown>): SwipeIssue | null {
  if (!parsePointText(a.from).ok) return "from_invalid";
  if (!parsePointText(a.to).ok) return "to_invalid";
  if (a.steps !== undefined && a.steps !== null && a.steps !== "") {
    const n = Number(a.steps);
    if (!Number.isInteger(n) || n < 1 || n > 1000) return "steps_invalid";
  }
  if (a.duration !== undefined && a.duration !== null && a.duration !== "") {
    const n = Number(a.duration);
    if (!Number.isInteger(n) || n < 0) return "duration_invalid";
  }
  return null;
}

/** 单个 action 合法结构 */
function isTouchAction(v: unknown): boolean {
  if (!isObj(v)) return false;
  const keys = Object.keys(v);
  if (keys.includes("move")) {
    if (keys.some((k) => k !== "move" && k !== "steps")) return false;
    if (!isPointArray(v.move)) return false;
    if (v.steps !== undefined) {
      const n = Number(v.steps);
      if (!Number.isInteger(n) || n < 1 || n > 1000) return false;
    }
    return true;
  }
  if (keys.length !== 1) return false;
  if ("down" in v) return isPointArray(v.down);
  if ("up" in v) return Array.isArray(v.up) && v.up.length === 0;
  if ("wait" in v) {
    if (!isObj(v.wait)) return false;
    const ms = Number(v.wait.ms);
    return Number.isInteger(ms) && ms >= 0;
  }
  return false;
}

/** 点数组 1..=5 */
function isPointArray(v: unknown): boolean {
  if (!Array.isArray(v) || v.length === 0 || v.length > 5) return false;
  return v.every(isPoint);
}

/** 点：{x,y} 或 {selector,align?}，可选 radius/force */
function isPoint(v: unknown): boolean {
  if (!isObj(v)) return false;
  if (v.selector !== undefined) {
    if (typeof v.selector !== "string" || v.selector.trim() === "") return false;
    if (v.x !== undefined || v.y !== undefined) return false;
    if (v.align !== undefined && v.align !== "center") return false;
  } else {
    if (typeof v.x !== "number" || typeof v.y !== "number") return false;
    if (!Number.isFinite(v.x) || !Number.isFinite(v.y)) return false;
  }
  for (const key of ["radius", "force"]) {
    if (v[key] !== undefined && (typeof v[key] !== "number" || !Number.isFinite(v[key]))) {
      return false;
    }
  }
  return true;
}

/** 普通对象 */
function isObj(v: unknown): v is Record<string, any> {
  return typeof v === "object" && v !== null && !Array.isArray(v);
}
