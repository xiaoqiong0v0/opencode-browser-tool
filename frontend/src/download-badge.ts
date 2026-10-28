// 工具栏下载角标的纯逻辑:只做计数/文案/脉冲判定,不碰 DOM 与 Tauri,便于单测
// 数据来源:`downloads-changed` 事件(触发刷新与脉冲)+ `/api/downloads` 快照(统计进行中数量)
// 注:相对导入带 `.ts` 扩展名 —— 本模块被 `node --test` 直接跑 TS 源(需显式扩展名才能解析)
import { parseDownloadsChanged } from "./download-events.ts";

/** 角标数字上限:超过显示 `99+`(避免窄按钮被长数字撑开) */
export const BADGE_MAX_COUNT = 99;

/** 角标 `title` 前缀(进行中的下载:`下载：<文件名>`) */
const TITLE_PREFIX = "下载：";

/** `/api/downloads` 快照中角标需要的部分(只读 `groups[].items[].state`) */
interface DownloadsSnapshotLike {
  groups?: { items?: { state?: unknown }[] }[];
}

/**
 * 统计快照中「进行中」(`state === "progress"`)的下载条数
 *
 * 结构按 `/api/downloads` 的 `{ groups: [{ dir, items: [...] }] }` 读取;
 * 缺失/类型不符的字段一律忽略(容错,不抛错)。
 *
 * @param snapshot `/api/downloads` 响应里的 `data`
 * @returns 进行中条数(无数据时为 0)
 */
export function countInProgressDownloads(snapshot: unknown): number {
  const groups = (snapshot as DownloadsSnapshotLike | null | undefined)?.groups;
  if (!Array.isArray(groups)) return 0;
  let count = 0;
  for (const group of groups) {
    const items = group?.items;
    if (!Array.isArray(items)) continue;
    for (const item of items) {
      if (item?.state === "progress") count += 1;
    }
  }
  return count;
}

/**
 * 角标文案:0 条进行中时返回空串(调用方据此隐藏角标),超过上限显示 `99+`
 *
 * @param count 进行中条数
 * @returns 角标文本;`""` 表示不显示
 */
export function downloadsBadgeText(count: number): string {
  if (!Number.isFinite(count) || count <= 0) return "";
  return count > BADGE_MAX_COUNT ? `${BADGE_MAX_COUNT}+` : String(Math.floor(count));
}

/**
 * 判断事件是否为「新下载开始」(应触发角标脉冲 + 更新 `title`)
 *
 * 只认合法 payload 且 `state === "progress"`(校验规则复用 `parseDownloadsChanged`)。
 *
 * @param payload `downloads-changed` 事件 data
 * @returns 新下载开始 → true
 */
export function isDownloadStart(payload: unknown): boolean {
  return parseDownloadsChanged(payload)?.state === "progress";
}

/**
 * 从事件提取按钮 `title` 文案(`下载：<filename>`)
 *
 * `filename` 缺失/空白时返回 null(调用方保持原标题不变)。
 *
 * @param payload `downloads-changed` 事件 data
 * @returns `下载：<filename>` 或 null
 */
export function downloadEventTitle(payload: unknown): string | null {
  const raw = payload as { filename?: unknown } | null | undefined;
  const name = typeof raw?.filename === "string" ? raw.filename.trim() : "";
  return name ? `${TITLE_PREFIX}${name}` : null;
}
