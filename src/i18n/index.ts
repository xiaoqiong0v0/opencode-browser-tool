import { getConfig, type Lang } from "../config/index.js";
import type { LocaleStrings, PanelStrings } from "./types.js";
import en from "./en.js";

export type { LocaleStrings, PanelStrings } from "./types.js";
export type { ToolStrings } from "./types.js";
export type { Lang } from "../config/index.js";

const _strings: Record<string, LocaleStrings> = {};

/** 注册语言包 */
export function registerLocale(lang: Lang, strings: LocaleStrings): void {
  _strings[lang] = strings;
}

/** 惰性兜底:任何入口(binary.ts 独立运行等)未走插件注册时至少有 en 可用,不输出裸键 */
function ensureFallback(): void {
  if (!_strings.en) _strings.en = en;
}

/** 获取工具描述翻译 */
export function t(key: string, lang?: Lang): string {
  ensureFallback();
  const l = lang || getConfig().toolLang;
  const str = _strings[l] || _strings.en;
  return str?.tool[key] || _strings.en?.tool[key] || key;
}

/** 取工具文案并替换 `{name}` 占位符(缺失键回退 en,再缺返回键名本身,不崩溃) */
export function tf(key: string, vars?: Record<string, string | number>, lang?: Lang): string {
  let s = t(key, lang);
  if (vars) {
    for (const [k, v] of Object.entries(vars)) s = s.split(`{${k}}`).join(String(v));
  }
  return s;
}

/** 获取面板 UI 翻译 */
export function panelT(key: string, lang?: Lang): string {
  ensureFallback();
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
