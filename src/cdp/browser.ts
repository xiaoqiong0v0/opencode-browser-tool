/**
 * CDP 浏览器控制器:封装浏览器进程启动 + 页面级 CDP 操作
 * 浏览器以 --app + --remote-debugging-port 启动,页面零注入
 */
import { spawn, type ChildProcess } from "child_process";
import { CdpClient } from "./client.js";

export interface BrowserOptions {
  executable: string;
  userDataDir: string;
  headless?: boolean;
  env?: Record<string, string>;
}

export interface BoxModel {
  x: number;
  y: number;
  width: number;
  height: number;
}

export class CdpBrowser {
  private proc: ChildProcess | null = null;
  private client = new CdpClient();
  private targetId: string | null = null;
  private port = 0;
  private pageReady: Promise<void> | null = null;

  get connected(): boolean {
    return this.client.isConnected;
  }

  /** 启动浏览器进程并连接调试端口 */
  async launch(opts: BrowserOptions, url: string): Promise<void> {
    // 端口 0 由系统分配,需先占用一个随机端口再释放给浏览器
    this.port = await this.pickFreePort();
    const args = [
      `--app=${url}`,
      `--remote-debugging-port=${this.port}`,
      `--user-data-dir=${opts.userDataDir}`,
      "--no-first-run",
      "--no-default-browser-check",
      "--disable-background-networking",
      "--no-sandbox",
    ];
    if (opts.headless) args.push("--headless=new");
    this.proc = spawn(opts.executable, args, {
      env: { ...process.env, ...opts.env },
      stdio: ["ignore", "ignore", "pipe"],
      windowsHide: false,
    });
    this.proc.stderr?.on("data", (d: Buffer) => {
      process.stderr.write("[browser] " + d.toString().trim() + "\n");
    });
    this.proc.on("exit", (code) => {
      process.stderr.write(`[browser] exited code=${code}\n`);
      this.client.close();
      this.targetId = null;
    });

    // 轮询调试端口直到可用
    const httpUrl = `http://127.0.0.1:${this.port}`;
    const deadline = Date.now() + 30000;
    let version: any = null;
    while (Date.now() < deadline) {
      if (this.proc?.exitCode !== null && this.proc?.exitCode !== undefined) {
        throw new Error(`Browser process exited early (code ${this.proc.exitCode})`);
      }
      try {
        const res = await fetch(`${httpUrl}/json/version`);
        version = await res.json();
        break;
      } catch {
        await new Promise((r) => setTimeout(r, 300));
      }
    }
    if (!version) throw new Error("Browser debug port not ready");
    // 浏览器级连接仅用于发现 target,页面操作需连接到 page target
    await this.client.connect(version.webSocketDebuggerUrl);
    const pageWs = await this.waitForPageTarget();
    // 重新连接到页面 target
    this.client.close();
    await this.client.connect(pageWs);
    await this.client.send("Page.enable").catch(() => {});
    // --app=URL 时页面可能仍在加载,等待就绪
    if (url && url !== "about:blank") {
      await this.waitForUrl(url);
    }
  }

  /** 等待页面加载到指定 URL(超时静默) */
  private async waitForUrl(url: string, timeoutMs = 15000): Promise<void> {
    const deadline = Date.now() + timeoutMs;
    while (Date.now() < deadline) {
      try {
        const cur = await this.evaluate("location.href");
        if (cur && String(cur).startsWith(url.split("#")[0])) return;
      } catch {}
      await new Promise((r) => setTimeout(r, 300));
    }
  }

  /** 等待页面 target 出现并返回其 WebSocket 地址 */
  private async waitForPageTarget(): Promise<string> {
    const deadline = Date.now() + 15000;
    while (Date.now() < deadline) {
      try {
        const list = await (await fetch(`http://127.0.0.1:${this.port}/json/list`)).json();
        const page = list.find((t: any) => t.type === "page" && !t.url.startsWith("devtools://"));
        if (page) {
          this.targetId = page.id;
          return page.webSocketDebuggerUrl;
        }
      } catch {}
      await new Promise((r) => setTimeout(r, 300));
    }
    throw new Error("No page target found");
  }

  /** 导航到指定 URL */
  async navigate(url: string): Promise<void> {
    await this.client.send("Page.enable");
    await this.client.send("Page.navigate", { url });
  }

  /** 在页面执行 JS,返回序列化结果 */
  async evaluate(expression: string): Promise<any> {
    const r = await this.client.send("Runtime.evaluate", {
      expression,
      returnByValue: true,
      awaitPromise: true,
    });
    if (r.exceptionDetails) throw new Error(r.exceptionDetails.text || "evaluate error");
    return r.result?.value;
  }

  /** 获取元素 box 模型(用于覆盖层绘制高亮) */
  async getBoxModel(selector: string): Promise<BoxModel | null> {
    const r = await this.evaluate(`(() => {
      const el = document.querySelector(${JSON.stringify(selector)});
      if (!el) return null;
      const r = el.getBoundingClientRect();
      return { x: r.x, y: r.y, width: r.width, height: r.height, scrollY: window.scrollY, scrollX: window.scrollX };
    })()`);
    if (!r) return null;
    return { x: r.x, y: r.y, width: r.width, height: r.height };
  }

  /** 页面截图(base64) */
  async screenshot(): Promise<string> {
    const r = await this.client.send("Page.captureScreenshot", { format: "png" });
    return r.data;
  }

  /** 当前页面 URL */
  async currentUrl(): Promise<string> {
    return (await this.evaluate("location.href")) || "";
  }

  /** 页面可见文本 */
  async visibleText(): Promise<string> {
    return (await this.evaluate("document.body ? document.body.innerText : ''")) || "";
  }

  async close(): Promise<void> {
    this.client.close();
    if (this.proc) {
      try { this.proc.kill(); } catch {}
      this.proc = null;
    }
  }

  private async pickFreePort(): Promise<number> {
    // 借用系统临时端口
    const net = await import("net");
    return new Promise((resolve, reject) => {
      const srv = net.createServer();
      srv.listen(0, "127.0.0.1", () => {
        const port = (srv.address() as any).port;
        srv.close(() => resolve(port));
      });
      srv.on("error", reject);
    });
  }
}
