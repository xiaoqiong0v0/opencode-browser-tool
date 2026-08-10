/** 工具描述文本字典 */
export interface ToolStrings {
  [key: string]: string;
}

/** 面板 UI 文本字典 */
export interface PanelStrings {
  [key: string]: string;
}

/** 完整语言包 */
export interface LocaleStrings {
  tool: ToolStrings;
  panel: PanelStrings;
}
