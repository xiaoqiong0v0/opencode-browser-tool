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
}
