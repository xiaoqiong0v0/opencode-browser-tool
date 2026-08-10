import { getConfig, type Lang } from "../config/index.js";
import type { LocaleStrings, PanelStrings } from "./types.js";

export type { LocaleStrings, PanelStrings } from "./types.js";
export type { ToolStrings } from "./types.js";
export type { Lang } from "../config/index.js";

const _strings: Record<string, LocaleStrings> = {};

/** 注册语言包 */
export function registerLocale(lang: Lang, strings: LocaleStrings): void {
  _strings[lang] = strings;
}

/** 获取工具描述翻译 */
export function t(key: string, lang?: Lang): string {
  const l = lang || getConfig().toolLang;
  const str = _strings[l] || _strings.en;
  return str?.tool[key] || _strings.en?.tool[key] || key;
}

/** 获取面板 UI 翻译 */
export function panelT(key: string, lang?: Lang): string {
  const l = lang || getConfig().panelLang;
  const str = _strings[l] || _strings.en;
  return str?.panel[key] || _strings.en?.panel[key] || key;
}

/** 获取面板完整翻译对象（供注入） */
export function getPanelStrings(lang?: Lang): PanelStrings {
  const l = lang || getConfig().panelLang;
  const str = _strings[l] || _strings.en;
  return str?.panel || ({} as PanelStrings);
}
