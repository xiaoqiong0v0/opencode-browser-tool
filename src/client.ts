import { spawn } from "child_process";
import { resolve, dirname } from "path";
import { existsSync } from "fs";
import { fileURLToPath } from "url";
import createLogger from "@xiaoqiong0v0/opencode-plugin-logger";

const __dirname = dirname(fileURLToPath(import.meta.url));

// Rust 二进制查找:dist/bin/pw-shell-{platform} 或环境变量覆盖
function resolveShellBinary(): string {
  const platform = process.platform === "win32" ? "win-x64" : "linux-x64";
  const candidates = [
    process.env.PW_SHELL_PATH,
    resolve(__dirname, "bin", `pw-shell-${platform}.exe`),
    resolve(__dirname, "bin", `pw-shell-${platform}`),
    resolve(__dirname, "..", "rust", "target", "debug", process.platform === "win32" ? "pw-shell.exe" : "pw-shell"),
  ];
  for (const c of candidates) {
    if (c && existsSync(c)) return c;
  }
  // 默认:开发环境用 cargo target
  return resolve(__dirname, "..", "rust", "target", "debug", process.platform === "win32" ? "pw-shell.exe" : "pw-shell");
}

let serviceProcess: any = null;
let servicePort = 0;
let serviceReady = false;

async function callApi(path: string, body?: any, sessionId?: string): Promise<any> {
  if (!serviceReady) throw new Error("Service not ready");
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
): Promise<void> {
  const log = createLogger("opencode-browser-tool");
  const shell = resolveShellBinary();
  const args = ["--browsers-path", browsersPath || ""];
  if (browserType) args.push("--browser", browserType);
  if (sessionIsolation) args.push("--session-isolation");
  serviceProcess = spawn(shell, args, { stdio: ["ignore", "pipe", "pipe"] });

  const port = await new Promise<number>((resolve, reject) => {
    const t = setTimeout(() => {
      reject(new Error("Service timeout"));
    }, 20000);
    serviceProcess.stdout.on("data", (data: Buffer) => {
      const line = data.toString().trim();
      const p = parseInt(line, 10);
      if (!isNaN(p)) {
        clearTimeout(t);
        resolve(p);
      }
    });
    serviceProcess.stderr.on("data", (data: Buffer) => {
      log.info("[shell] " + data.toString().trim());
    });
    serviceProcess.on("error", (e: Error) => {
      clearTimeout(t);
      reject(new Error(`Failed to start pw-shell: ${e.message}. Build it with: cd rust && cargo build --release`));
    });
    serviceProcess.on("exit", (code: number) => {
      clearTimeout(t);
      if (!serviceReady) reject(new Error(`Service exited ${code}`));
    });
  });

  servicePort = port;
  serviceReady = true;
  log.info(`Shell service ready on port ${port} (${shell})`);
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
};

