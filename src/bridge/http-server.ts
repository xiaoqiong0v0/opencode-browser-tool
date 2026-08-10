import { createServer, IncomingMessage, ServerResponse } from "http";
import type { PickData, BridgeMessageHandler } from "../types.js";

interface Record {
  id: number;
  type: "annotate" | "screenshot";
  tag?: string;
  selector?: string;
  content?: string;
  pageUrl: string;
  sessionId?: string;
  annotation: string;
  base64?: string;
  fullBase64?: string;
}

export type ScreenshotHandler = (rect?: { x: number; y: number; w: number; h: number }) => Promise<string>;

export class HttpBridge {
  private server: ReturnType<typeof createServer> | null = null;
  private handler: BridgeMessageHandler | null = null;
  private shotHandler: ScreenshotHandler | null = null;
  private sentHandler: ((records: Record[]) => void) | null = null;
  private port: number;
  private records = new Map<number, Record>();
  private lastSent: Record[] = [];
  private nextId = 1;

  private allocId(): number {
    const id = this.nextId++;
    if (this.nextId > Number.MAX_SAFE_INTEGER - 1000) this.nextId = 1;
    return id;
  }

  constructor(port: number = 3456) {
    this.port = port;
  }

  start(): Promise<number> {
    return new Promise((resolve, reject) => {
      this.server = createServer((req, res) => {
        res.setHeader("Access-Control-Allow-Origin", "*");
        res.setHeader("Access-Control-Allow-Methods", "GET, POST, OPTIONS");
        res.setHeader("Access-Control-Allow-Headers", "Content-Type");
        if (req.method === "OPTIONS") {
          res.writeHead(204);
          res.end();
          return;
        }

        if (req.method === "POST") {
          let body = "";
          req.on("data", (c: Buffer) => (body += c.toString()));
          req.on("end", async () => {
            try {
              const data = JSON.parse(body);
              res.writeHead(200, { "Content-Type": "application/json" });
              res.end(JSON.stringify(await this.route(req.url || "/", data)));
            } catch (e: any) {
              res.writeHead(400);
              res.end(JSON.stringify({ error: e?.message || "invalid" }));
            }
          });
          return;
        }

        if (req.method === "GET") {
          res.writeHead(200, { "Content-Type": "application/json" });
          res.end(JSON.stringify(this.handleGet(req.url || "/")));
          return;
        }

        res.writeHead(405);
        res.end();
      });
      this.server.on("error", (err: Error) => {
        if ((err as NodeJS.ErrnoException).code === "EADDRINUSE") reject(new Error(`端口 ${this.port} 已被占用`));
        else reject(err);
      });
      this.server.listen(this.port, "127.0.0.1", () => {
        const addr = this.server!.address();
        if (addr && typeof addr === "object") this.port = addr.port;
        resolve(this.port);
      });
    });
  }

  stop(): Promise<void> {
    return new Promise((r) => {
      if (this.server) {
        this.server.close(() => r());
        this.server = null;
      } else r();
    });
  }

  onMessage(h: BridgeMessageHandler) {
    this.handler = h;
  }
  onScreenshot(h: ScreenshotHandler) {
    this.shotHandler = h;
  }
  onSent(h: (records: Record[]) => void) {
    this.sentHandler = h;
  }

  getRecords(): Record[] {
    return Array.from(this.records.values()).sort((a, b) => a.id - b.id);
  }
  clearRecords(): void {
    this.records.clear();
    if (this.nextId > Number.MAX_SAFE_INTEGER - 1000) this.nextId = 1;
  }

  private handleGet(url: string): any {
    const [path, qs] = url.split("?");
    const sp = new URLSearchParams(qs || "");
    if (path === "/records") {
      const sessionId = sp.get("sessionId") || "";
      const showAll = sp.get("all") === "true";
      const active = new Set(this.getRecords().map((r) => r.id));
      const pool = showAll ? [...active].map((id) => this.records.get(id)!).concat(this.lastSent) : this.getRecords();
      const list = pool
        .filter((r) => r && (!sessionId || !r.sessionId || r.sessionId === sessionId))
        .map((r) => ({ ...r, sent: !active.has(r.id), ...(r.type === "screenshot" ? { fullBase64: undefined } : {}) }));
      return { success: true, records: list };
    }
    const rm = path.match(/^\/record\/(\d+)$/);
    if (rm) {
      const id = Number(rm[1]);
      let rec = this.records.get(id);
      if (!rec) rec = this.lastSent.find((r) => r.id === id);
      return rec ? { success: true, record: rec } : { success: false, error: "not found" };
    }
    return { success: false };
  }

  private async route(url: string, data: any): Promise<any> {
    switch (url) {
      case "/annotate": {
        const id = this.allocId();
        this.records.set(id, {
          id,
          type: "annotate",
          tag: data.tag || "",
          selector: data.selector || "",
          content: data.content || "",
          pageUrl: data.pageUrl || "",
          sessionId: data.sessionId || "",
          annotation: "",
        });
        return { success: true, id };
      }
      case "/annotate/update": {
        const rec = this.records.get(data.id);
        if (rec) rec.annotation = data.annotation || "";
        return { success: true };
      }
      case "/annotate/discard": {
        this.records.delete(data.id);
        return { success: true };
      }
      case "/send-all": {
        const all = this.getRecords();
        if (this.handler && all.length > 0) {
          const old = this.lastSent;
          this.lastSent = all; // 存档，供 LLM 通过 id 读取
          if (this.sentHandler && old.length > 0) try { this.sentHandler(old); } catch {}
          const elements = all.map((r) => ({
            id: r.id,
            type: r.type,
            tag: r.type === "annotate" ? r.tag : undefined,
            pageUrl: r.pageUrl,
            annotation: r.annotation,
            sessionId: r.sessionId,
          }));
          try { this.handler({ action: "pick", elements, pageUrl: all[0]?.pageUrl || "" }); } catch {}
        }
        this.clearRecords(); // 清空当前记录
        return { success: true, count: all.length };
      }
      case "/screenshot": {
        if (data.store) {
          const id = this.allocId();
          this.records.set(id, {
            id,
            type: "screenshot",
            pageUrl: data.pageUrl || "",
            sessionId: data.sessionId || "",
            annotation: data.note || "",
            base64: data.base64,
            fullBase64: data.fullBase64,
          });
          return { success: true, id };
        }
        if (this.shotHandler) {
          const base64 = await this.shotHandler(data.rect);
          return { success: true, base64 };
        }
        return { success: false, error: "no screenshot handler" };
      }
      default:
        return { success: false };
    }
  }
}
