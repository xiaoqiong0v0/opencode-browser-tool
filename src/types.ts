/** 面板发送的标注/截图数据 */
export interface PickData {
  action: "pick" | "cancel";
  elements: {
    id: number;
    type?: string;
    tag?: string;
    pageUrl?: string;
    annotation?: string;
    sessionId?: string;
  }[];
  pageUrl: string;
}

/** HTTP Bridge 消息回调 */
export type BridgeMessageHandler = (data: PickData) => void | Promise<void>;

/** 调用端点返回的标注记录 */
export interface AnnotationRecord {
  index: number;
  selector: string;
  tagName: string;
  text: string;
  pageUrl: string;
  annotation: string;
}
