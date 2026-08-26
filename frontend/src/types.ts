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

/** 批注记录(面板展示) */
export interface AnnotationRecord {
  index: number;
  selector: string;
  rect: { x: number; y: number; w: number; h: number };
  note: string;
}

/** 覆盖层绘制命令(window.__btOverlay.redraw) */
export interface OverlayApi {
  redraw(rects: HighlightRect[], marks: AnnotationMark[]): void;
  /** 显示通知气泡(message/type: ok|bad|err) */
  notify(message: string, type?: string): void;
  /** 在坐标处显示批注输入弹框(点击元素后调用) */
  showNoteInput(x: number, y: number, selector: string): void;
  /** 显示/隐藏面板遮罩(面板打开时覆盖页面区,点击关闭) */
  showMask(): void;
  hideMask(): void;
}
