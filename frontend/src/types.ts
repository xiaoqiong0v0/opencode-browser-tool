// 覆盖层数据类型:Rust → 覆盖层 Webview 的绘制数据

/** 高亮矩形 */
export interface HighlightRect {
  x: number;
  y: number;
  w: number;
  h: number;
  color: number; // 0x00RRGGBB
}

/** 批注标记 */
export interface AnnotationMark {
  x: number;
  y: number;
  w: number;
  h: number;
  index: number;
  color: number;
}

/** 记录类型 */
export type RecordType = "annotate" | "screenshot";

/** 批注/截图记录(面板展示) */
export interface AnnotationRecord {
  index: number;
  /** annotate(批注) / screenshot(截图) */
  type: RecordType;
  selector: string;
  rect: { x: number; y: number; w: number; h: number };
  note: string;
  /** 截图记录:Png base64 data URL(仅截图类型) */
  image?: string;
}

/** 覆盖层绘制命令(window.__btOverlay 暴露给 Rust 调用) */
export interface OverlayApi {
  redraw(rects: HighlightRect[], marks: AnnotationMark[]): void;
  /** 显示通知气泡(message/type: ok|bad|err) */
  notify(message: string, type?: string): void;
  /** 在坐标处显示批注输入弹框(点击元素后调用) */
  showNoteInput(x: number, y: number, selector: string): void;
  /** 设置覆盖层模式:annotate(批注) / shot(截图) / none */
  setMode(mode: "none" | "annotate" | "shot"): void;
  /** 显示截图预览(dataUrl) + 底部工具栏 */
  showShotPreview(dataUrl: string): void;
  /** 隐藏截图预览 */
  hideShotPreview(): void;
  /** 显示/隐藏面板遮罩(面板打开时覆盖页面区,点击关闭) */
  showMask(): void;
  hideMask(): void;
}
