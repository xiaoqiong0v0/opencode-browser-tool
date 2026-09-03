import { spawn } from "child_process";
import { resolve, dirname } from "path";
import { existsSync } from "fs";
import { fileURLToPath } from "url";
import createLogger from "@xiaoqiong0v0/opencode-plugin-logger";

const __dirname = dirname(fileURLToPath(import.meta.url));

// Rust 二进制查找:dist/bin/bt-shell-{platform} 或环境变量覆盖
function resolveShellBinary(): string {
  const platform = process.platform === "win32" ? "win-x64" : "linux-x64";
  const candidates = [
    process.env.BT_SHELL_PATH,
    resolve(__dirname, "bin", `bt-shell-${platform}.exe`),
    resolve(__dirname, "bin", `bt-shell-${platform}`),
    resolve(__dirname, "..", "rust", "target", "release", process.platform === "win32" ? "bt-shell.exe" : "bt-shell"),
    resolve(__dirname, "..", "rust", "target", "debug", process.platform === "win32" ? "bt-shell.exe" : "bt-shell"),
  ];
  for (const c of candidates) {
    if (c && existsSync(c)) return c;
  }
  // 默认:开发环境用 cargo target 产物(release 优先)
  return resolve(__dirname, "..", "rust", "target", "release", process.platform === "win32" ? "bt-shell.exe" : "bt-shell");
}

let serviceProcess: any = null;
let servicePort = 0;
let serviceReady = false;
/** 服务启动参数(configureService 保存,ensureService 懒启动时使用) */
let serviceConfig: { nodePath: string; browsersPath?: string; sessionIsolation?: boolean; browserType?: string; userDataDir?: string } | null = null;
/** 正在启动中的 Promise(并发保护,多个工具同时调用只启动一次) */
let starting: Promise<void> | null = null;

/** 保存服务启动参数(插件加载时调用,不立即启动窗口) */
export function configureService(opts: { nodePath: string; browsersPath?: string; sessionIsolation?: boolean; browserType?: string; userDataDir?: string }): void {
  serviceConfig = opts;
}

/** 运行时切换用户数据目录(下次 ensureService 重启时生效) */
export function setUserDataDir(dir: string): void {
  if (serviceConfig) serviceConfig.userDataDir = dir;
}

/** 懒启动服务:未启动则 spawn bt-shell(弹窗);已启动直接返回;并发时复用同一个 Promise */
export async function ensureService(): Promise<void> {
  if (serviceReady) return;
  if (!serviceConfig) throw new Error("Service not configured");
  if (starting) return starting;
  starting = startService(
    serviceConfig.nodePath,
    serviceConfig.browsersPath,
    serviceConfig.sessionIsolation,
    serviceConfig.browserType,
    serviceConfig.userDataDir,
  ).finally(() => {
    starting = null;
  });
  return starting;
}

async function callApi(path: string, body?: any, sessionId?: string): Promise<any> {
  // 懒启动:首次调用工具时自动打开窗口
  await ensureService();
  const payload = { ...(body || {}), _sessionId: sessionId || "" };
  const res = await fetch(`http://127.0.0.1:${servicePort}${path}`, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify(payload),
  });
  const json = await res.json();
  if (!json.success) throw new Error(json.error || "Service error");
  return json.data;
}

export async function startService(
  nodePath: string,
  browsersPath?: string,
  sessionIsolation?: boolean,
  browserType?: string,
  userDataDir?: string,
): Promise<void> {
  const log = createLogger("opencode-browser-tool");
  const shell = resolveShellBinary();
  // 显式指定端口(release 模式 GUI 程序无控制台,无法解析 stdout)
  const port = 18000 + Math.floor(Math.random() * 1000);
  const args = ["--browsers-path", browsersPath || "", "--port", String(port)];
  if (browserType) args.push("--browser", browserType);
  if (sessionIsolation) args.push("--session-isolation");
  // 多用户配置:userDataDir 指向独立 WebView2 用户数据目录
  if (userDataDir) args.push("--user-data-dir", userDataDir);
  serviceProcess = spawn(shell, args, { stdio: ["ignore", "pipe", "pipe"] });

  // 等待服务就绪(轮询端口)
  const deadline = Date.now() + 20000;
  while (Date.now() < deadline) {
    try {
      const res = await fetch(`http://127.0.0.1:${port}/api/status`, {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: "{}",
      });
      if (res.ok) {
        servicePort = port;
        serviceReady = true;
        log.info(`Shell service ready on port ${port} (${shell})`);
        return;
      }
    } catch {
      // 服务未启动,继续等待
    }
    await new Promise((r) => setTimeout(r, 300));
  }
  throw new Error("Service timeout: bt-shell did not start. Build it with: cd rust && cargo build --release");
}

export async function stopService(): Promise<void> {
  if (serviceProcess) {
    try {
      serviceProcess.kill("SIGTERM");
    } catch {}
    serviceProcess = null;
  }
  serviceReady = false;
  servicePort = 0;
}

/** 打开浏览器窗口(未启动则启动;已启动幂等) */
export async function openWindow(): Promise<void> {
  await ensureService();
}

/** 服务当前是否运行 */
export function isRunning(): boolean {
  return serviceReady;
}

function cmd(name: string) {
  return (params?: any, sessionId?: string) => callApi(`/api/${name}`, params, sessionId);
}

export const service = {
  navigate: cmd("navigate"),
  click: cmd("click"),
  fill: cmd("fill"),
  clear: cmd("clear"),
  select: cmd("select"),
  hover: cmd("hover"),
  pressKey: cmd("press-key"),
  drag: cmd("drag"),
  uploadFile: cmd("upload-file"),
  screenshot: cmd("screenshot"),
  evaluate: cmd("evaluate"),
  visibleText: cmd("visible-text"),
  visibleHtml: cmd("visible-html"),
  elementState: cmd("element-state"),
  dropdownOptions: cmd("dropdown-options"),
  waitForSelector: cmd("wait-for-selector"),
  scroll: cmd("scroll"),
  scrollToElement: cmd("scroll-to-element"),
  reload: cmd("reload"),
  goBack: cmd("go-back"),
  goForward: cmd("go-forward"),
  resize: cmd("resize"),
  device: cmd("device"),
  deviceList: cmd("device/list"),
  devtools: cmd("devtools"),
  mediaMode: cmd("media/mode"),
  mediaAudio: cmd("media/audio"),
  mediaVideo: cmd("media/video"),
  close: cmd("close"),
  newTab: cmd("tabs/new"),
  switchTab: cmd("tabs/switch"),
  closeTab: cmd("tabs/close"),
  clickSwitchTab: cmd("click-switch-tab"),
  iframeClick: cmd("iframe-click"),
  iframeFill: cmd("iframe-fill"),
  userAgent: cmd("user-agent"),
  accessibility: cmd("accessibility"),
  notify: cmd("notify"),
  pdf: cmd("pdf"),
  consoleLogs: cmd("console-logs"),
  expectResponse: cmd("expect-response"),
  assertResponse: cmd("assert-response"),
  panelConfig: cmd("panel-config"),
  closeSession: cmd("close-session"),
  status: cmd("status"),
  tabs: cmd("tabs"),
  annotateToggle: cmd("annotate/toggle"),
  annotateRecords: cmd("annotate/records"),
  annotateSend: cmd("annotate/send"),
  annotateConsumeSent: cmd("annotate/consume-sent"),
};

