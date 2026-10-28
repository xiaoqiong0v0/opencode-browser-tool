// 下载事件(`downloads-changed`)的纯逻辑:只做判定、不碰 DOM,便于单测
// Rust 侧契约:新下载开始只发一次 `{ id, filename, state: "progress" }`,
// 结束态(completed/failed/canceled)发 `{ id, state }` 且只发一次;
// **完成态额外带 `filename`/`path`**(另存为只有这里能拿到最终文件名)。
// 面板侧契约:**完全不打扰用户** —— 不改面板可见性、不切换视图、不滚动、不展开分组;
// 只有用户当前正停在「下载」视图时才刷新一次列表(节流由调用方负责)。
// 工具栏角标的计数/脉冲判定见 `download-badge.ts`。

/** 下载状态(与 Rust `/api/downloads` 的 camelCase 字段一致) */
export type DownloadStateName = "progress" | "completed" | "failed" | "canceled";

/** Rust `downloads-changed` 事件 payload */
export interface DownloadsChangedPayload {
  id: string;
  /** 新下载开始:最终落盘文件名;**完成**:同一个文件名(用于另存为/工具栏提示) */
  filename?: string;
  /** 仅**完成**时携带:最终落盘绝对路径 */
  path?: string;
  state: DownloadStateName;
}

/** 面板内容区视图(记录 / 下载) */
export type PanelViewName = "records" | "downloads";

/** 事件处理动作:`refresh` = 刷新列表;`none` = 什么都不做 */
export type DownloadsChangedAction = "refresh" | "none";

/** 事件处理决策 */
export interface DownloadsChangedDecision {
  id: string;
  state: DownloadStateName;
  /** 仅当用户当前处于「下载」视图时为 `refresh`,其余为 `none` */
  action: DownloadsChangedAction;
}

/** 校验通过的 `downloads-changed` 事件(只保留决策需要的字段) */
export interface DownloadsChangedEvent {
  id: string;
  state: DownloadStateName;
}

/**
 * 校验并提取 `downloads-changed` payload 的核心字段
 *
 * @param payload 事件数据(容忍缺失/未知状态的宽松 payload)
 * @returns `{id, state}`;payload 非法(id 非非空字符串 / 状态未知)时返回 null
 */
export function parseDownloadsChanged(payload: unknown): DownloadsChangedEvent | null {
  const raw = payload as Partial<DownloadsChangedPayload> | null | undefined;
  const id = typeof raw?.id === "string" ? raw.id : "";
  const state = raw?.state;
  if (!id) return null;
  if (state !== "progress" && state !== "completed" && state !== "failed" && state !== "canceled") {
    return null;
  }
  return { id, state };
}

/**
 * 解析 `downloads-changed` payload 并给出处理决策
 *
 * - `activeView === "downloads"` → `refresh`(用户正看着下载列表,刷新一次有意义);
 * - 其余视图(记录 / 面板关闭 / 配置页)→ `none`,连刷新都不做,避免打断用户。
 *
 * 决策**不含**任何视图切换/展开/滚动字段:事件不改动面板与列表的呈现状态。
 *
 * @param payload 事件数据(容忍缺失/未知状态的宽松 payload)
 * @param activeView 用户当前所在的内容区视图
 * @returns 决策对象;payload 非法时返回 null(调用方直接忽略)
 */
export function decideDownloadsChanged(
  payload: unknown,
  activeView: PanelViewName,
): DownloadsChangedDecision | null {
  const event = parseDownloadsChanged(payload);
  if (!event) return null;
  return { ...event, action: activeView === "downloads" ? "refresh" : "none" };
}
