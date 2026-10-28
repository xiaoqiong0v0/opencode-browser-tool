// 地址栏的纯逻辑:失焦恢复判定 + 新标签页地址识别
// 不碰 DOM 与 Tauri,便于单测(`node --test` 直接跑 TS 源,故导入方需带 `.ts` 扩展名)

/** 新标签页(内部页)地址特征:about:blank / newtab.html / tauri 内部来源 */
const NEWTAB_URL_MARKERS = ["about:blank", "/newtab.html", "tauri://localhost", "tauri.localhost"] as const;

/**
 * 判断是否新标签页地址(about:blank / tauri 内部 newtab 地址),地址栏应显示为空
 *
 * @param url 标签页 URL
 * @returns 内部新标签页 → true
 */
export function isNewTabUrl(url: string): boolean {
  return NEWTAB_URL_MARKERS.some((marker) => url === marker || url.includes(marker));
}

/**
 * 地址栏失焦(blur)时的恢复判定
 *
 * 规则:输入为空(含仅空白)→ 回填当前激活标签的 URL;其余情况一律不干预,避免覆盖用户输入。
 * 拿不到可用 URL(缺失 / 空白 / 新标签页内部地址)时返回 null,调用方保持原样不动。
 *
 * @param input 地址栏当前值
 * @param activeUrl 当前激活标签的 URL(未知时传 null / undefined)
 * @returns 需要写回地址栏的 URL;null 表示保持用户输入不动
 */
export function resolveAddrBlurValue(input: string, activeUrl: string | null | undefined): string | null {
  if (input.trim() !== "") return null;
  const url = typeof activeUrl === "string" ? activeUrl.trim() : "";
  if (!url || isNewTabUrl(url)) return null;
  return url;
}
