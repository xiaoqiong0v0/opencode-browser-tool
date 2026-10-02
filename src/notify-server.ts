// 插件侧通知监听:shell 发送记录后主动 POST /notify 到这里(推送式投递,替代轮询)。
// 仅绑定 127.0.0.1 + 随机空闲端口;端点只接收最小信息(count),大内容仍由插件走
// /api/annotate/consume-sent 拉取(drain)。
import { createServer, type Server } from "node:http";

let server: Server | null = null;
let port = 0;
let onNotify: (() => void) | null = null;

/** 启动监听(幂等);返回实际端口。onNotify 在收到 shell 通知时被调用 */
export async function startNotifyServer(handler: () => void): Promise<number> {
  onNotify = handler;
  if (server && port) return port;
  return new Promise<number>((resolvePort, reject) => {
    const srv = createServer((req, res) => {
      if (req.method !== "POST" || !req.url || !req.url.startsWith("/notify")) {
        res.writeHead(404, { "content-type": "application/json" });
        res.end('{"ok":false}');
        return;
      }
      // 只读取/丢弃最小 payload,避免大内容经此通道
      let bytes = 0;
      req.on("data", (c: Buffer) => {
        bytes += c.length;
        if (bytes > 4096) req.destroy();
      });
      req.on("end", () => {
        res.writeHead(200, { "content-type": "application/json" });
        res.end('{"ok":true}');
        try {
          onNotify?.();
        } catch {
          // 忽略回调异常,不影响响应
        }
      });
    });
    srv.on("error", reject);
    // 不阻塞插件进程退出
    if (typeof (srv as any).unref === "function") (srv as any).unref();
    srv.listen(0, "127.0.0.1", () => {
      server = srv;
      const addr = srv.address();
      port = typeof addr === "object" && addr ? addr.port : 0;
      resolvePort(port);
    });
  });
}

/** 当前通知地址(shell 用它 POST);未就绪返回 null */
export function getNotifyUrl(): string | null {
  return server && port ? `http://127.0.0.1:${port}/notify` : null;
}
