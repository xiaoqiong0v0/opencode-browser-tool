/**
 * 原始 CDP 客户端:通过 WebSocket 直连浏览器调试端口
 * 使用 Node 24+ 内置 WebSocket,无第三方依赖
 */
export interface CdpEvent {
  method: string;
  params: any;
}

export class CdpClient {
  private ws: WebSocket | null = null;
  private msgId = 0;
  private pending = new Map<number, { resolve: (v: any) => void; reject: (e: Error) => void }>();
  private listeners = new Map<string, Set<(params: any) => void>>();
  private closed = false;

  /** 连接调试 WebSocket 地址 */
  connect(wsUrl: string): Promise<void> {
    // 关闭可能残留的旧连接
    if (this.ws) {
      const old = this.ws;
      this.ws = null;
      try { old.close(); } catch {}
    }
    this.closed = false;
    return new Promise((resolve, reject) => {
      const ws = new WebSocket(wsUrl);
      const t = setTimeout(() => {
        reject(new Error("CDP connect timeout: " + wsUrl));
        try { ws.close(); } catch {}
      }, 10000);
      ws.onopen = () => {
        clearTimeout(t);
        this.ws = ws;
        this.closed = false;
        resolve();
      };
      ws.onerror = () => {
        clearTimeout(t);
        reject(new Error("CDP connect failed: " + wsUrl));
      };
      ws.onmessage = (ev: MessageEvent) => {
        const data = JSON.parse(String(ev.data));
        if (data.id) {
          const p = this.pending.get(data.id);
          if (!p) return;
          this.pending.delete(data.id);
          if (data.error) p.reject(new Error(data.error.message || "CDP error"));
          else p.resolve(data.result);
        } else if (data.method) {
          const set = this.listeners.get(data.method);
          if (set) for (const fn of [...set]) {
            try { fn(data.params); } catch (e) { console.error("[cdp] listener error:", e); }
          }
        }
      };
      ws.onclose = () => {
        // 旧连接关闭不污染新连接状态
        if (this.ws !== ws) return;
        this.closed = true;
        this.ws = null;
        for (const p of this.pending.values()) p.reject(new Error("CDP connection closed"));
        this.pending.clear();
      };
    });
  }

  get isConnected(): boolean {
    return !!this.ws && !this.closed;
  }

  /** 发送 CDP 命令并等待结果 */
  send(method: string, params: any = {}): Promise<any> {
    if (!this.ws || this.closed) return Promise.reject(new Error("CDP not connected"));
    const id = ++this.msgId;
    return new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
      this.ws!.send(JSON.stringify({ id, method, params }));
    });
  }

  /** 订阅事件(返回取消函数) */
  on(method: string, fn: (params: any) => void): () => void {
    if (!this.listeners.has(method)) this.listeners.set(method, new Set());
    const set = this.listeners.get(method)!;
    set.add(fn);
    return () => set.delete(fn);
  }

  close(): void {
    const ws = this.ws;
    this.ws = null;
    this.closed = true;
    this.listeners.clear();
    try { ws?.close(); } catch {}
  }
}
