// 插件侧通知监听:shell 发送记录后主动 POST /notify[/<sid>] 到这里(推送式投递,替代轮询)。
// 仅绑定 127.0.0.1 + 随机空闲端口;端点只接收最小信息(count),大内容仍由插件走
// /api/annotate/consume-sent 拉取(drain)。
//
// 1.9.0 会话隔离:每个 shell 实例注册自己的 `/notify/<sid>`,通知按 URL 里的 sid 路由到
// 对应会话的 drain —— 彻底取代旧的「最后活跃会话」(last-writer-wins)投递。
import { createServer, type Server } from "node:http";
import { log } from "./logger.js";

let server: Server | null = null;
let port = 0;
/** 收到通知时的回调:参数为通知 URL 里的 sid(空串 = 未带 sid 的旧格式 /notify) */
let onNotify: ((sid: string) => void) | null = null;

/** 启动监听(幂等);返回实际端口。onNotify 在收到 shell 通知时被调用(带 sid) */
export async function startNotifyServer(handler: (sid: string) => void): Promise<number> {
  onNotify = handler;
  if (server && port) return port;
  return new Promise<number>((resolvePort, reject) => {
    const srv = createServer((req, res) => {
      const url = req.url || "";
      if (req.method !== "POST" || !url.startsWith("/notify")) {
        res.writeHead(404, { "content-type": "application/json" });
        res.end('{"ok":false}');
        return;
      }
      // 从路径提取 sid:/notify/<sid>;裸 /notify(旧格式)→ sid = ""
      // 非法百分号编码不能让异常冒到 request listener 外(会打崩进程),按无 sid 处理
      let sid = "";
      if (url.startsWith("/notify/")) {
        try {
          sid = decodeURIComponent(url.slice("/notify/".length));
        } catch {
          sid = "";
          // 公共 logger 无 warn()(只有 info/error),降级用 info 并显式标注 [warn]
          log.info(`[notify][warn] malformed sid in url (undecodable), treated as empty: ${url.slice(0, 120)}`);
        }
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
          onNotify?.(sid);
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

/** 某会话的通知地址:每个 shell 实例注册各自这条 → 通知可按 sid 归属 */
export function getNotifyUrlForSession(sid: string): string | null {
  return server && port ? `http://127.0.0.1:${port}/notify/${encodeURIComponent(sid)}` : null;
}
