import { readFileSync, writeFileSync, readdirSync, unlinkSync, mkdirSync, rmSync, existsSync } from "fs";
import { resolve } from "path";
import stringArgv from "string-argv";
import { parseArgs } from "node:util";
import {
  loadConfig, getConfig, getBrowsersDir, getCacheDir, getProfilesDir, getProfileDir,
  getExportsDir, getTmpDir, getActiveProfile, setActiveProfile,
} from "./config/index.js";
import { registerLocale, t } from "./i18n/index.js";
import en from "./i18n/en.js";
import zh from "./i18n/zh.js";
import { configureService, setUserDataDir, ensureService, openWindow, isRunning, stopService, service } from "./client.js";
import { tool, type Plugin } from "@opencode-ai/plugin";
import createLogger from "@xiaoqiong0v0/opencode-plugin-logger";

const log = createLogger("browser-tool");
const _t = (k: string) => t(k);

/** CLI 命令参数定义:flag=参数名,type=类型(string/boolean) */
type CmdArg = { flag: string; type?: "string" | "boolean" };
/** CLI 命令定义:usage=参数用法,descKey=命令描述 i18n 键(cli.cmd.*),args=可解析的 flag 表,run=执行 */
type CmdDef = {
  usage: string;
  descKey: string;
  args?: CmdArg[];
  run: (a: any) => Promise<any>;
};

/** 命令表:bt_cli 按 command 分发;命令描述全部走 i18n 键,帮助按 toolLang 本地化 */
const COMMANDS: Record<string, CmdDef> = {
  navigate: { usage: "--url", descKey: "cli.cmd.navigate", args: [{ flag: "url" }], run: async (a) => {
    const r = await service.navigate(a);
    if (r.installError) return `Installation failed: ${r.error}. Retry.`;
    return r.installing ? `Installing ${r.installing} (${r.progress}). Try again.` : `Navigated to: ${r.url}`;
  } },
  click: { usage: "--selector", descKey: "cli.cmd.click", args: [{ flag: "selector" }], run: async (a) => { await service.click(a); return "Clicked"; } },
  fill: { usage: "--selector --value", descKey: "cli.cmd.fill", args: [{ flag: "selector" }, { flag: "value" }], run: async (a) => { await service.fill(a); return "Filled"; } },
  clear: { usage: "--selector", descKey: "cli.cmd.clear", args: [{ flag: "selector" }], run: async (a) => { await service.clear(a); return "Cleared"; } },
  select: { usage: "--selector --value", descKey: "cli.cmd.select", args: [{ flag: "selector" }, { flag: "value" }], run: async (a) => { await service.select(a); return "Selected"; } },
  hover: { usage: "--selector", descKey: "cli.cmd.hover", args: [{ flag: "selector" }], run: async (a) => { await service.hover(a); return "Hovered"; } },
  drag: { usage: "--sourceSelector --targetSelector", descKey: "cli.cmd.drag", args: [{ flag: "sourceSelector" }, { flag: "targetSelector" }], run: async (a) => { await service.drag(a); return "Dragged"; } },
  press_key: { usage: "--key [--selector]", descKey: "cli.cmd.press_key", args: [{ flag: "key" }, { flag: "selector" }], run: async (a) => { await service.pressKey(a); return `Pressed: ${a.key}`; } },
  upload_file: { usage: "--selector --filePath", descKey: "cli.cmd.upload_file", args: [{ flag: "selector" }, { flag: "filePath" }], run: async (a) => { await service.uploadFile(a); return "Uploaded"; } },
  screenshot: { usage: "[--selector]", descKey: "cli.cmd.screenshot", args: [{ flag: "selector" }], run: async (a) => {
    const r = await service.screenshot(a);
    return { output: `Screenshot taken${a.selector ? ` (element: ${a.selector})` : " (full page)"}`, attachments: [{ type: "file", mime: r.mime || "image/png", data: r.base64, url: "" }] };
  } },
  evaluate: { usage: "--script", descKey: "cli.cmd.evaluate", args: [{ flag: "script" }], run: async (a) => JSON.stringify(await service.evaluate(a), null, 2) },
  get_visible_text: { usage: "[--selector]", descKey: "cli.cmd.get_visible_text", args: [{ flag: "selector" }], run: async (a) => (await service.visibleText(a))?.text || "(no visible text)" },
  get_visible_html: { usage: "[--selector --removeScripts --removeComments --maxLength]", descKey: "cli.cmd.get_visible_html", args: [{ flag: "selector" }, { flag: "removeScripts", type: "boolean" }, { flag: "removeComments", type: "boolean" }, { flag: "maxLength" }], run: async (a) => await service.visibleHtml(a) },
  console_logs: { usage: "[--type --search --limit --clear]", descKey: "cli.cmd.console_logs", args: [{ flag: "type" }, { flag: "search" }, { flag: "limit" }, { flag: "clear", type: "boolean" }], run: async (a) => { const r = await service.consoleLogs(a); return (r.logs || []).join("\n") || "(no logs)"; } },
  go_back: { usage: "", descKey: "cli.cmd.go_back", run: async () => { const r = await service.goBack(); return `Went back, current URL: ${r.url}`; } },
  go_forward: { usage: "", descKey: "cli.cmd.go_forward", run: async () => { const r = await service.goForward(); return `Went forward, current URL: ${r.url}`; } },
  resize: { usage: "--width --height", descKey: "cli.cmd.resize", args: [{ flag: "width" }, { flag: "height" }], run: async (a) => { await service.resize(a); return "Resized"; } },
  set_device: { usage: "[--name]", descKey: "cli.cmd.set_device", args: [{ flag: "name" }], run: async (a) => {
    if (!a.name) {
      const list = await service.deviceList();
      return ["Available device presets:", "", ...(list.devices || []).map((d: any) => `  ${d.name}  ${d.width}x${d.height}${d.ua ? `  ${d.ua.slice(0, 60)}...` : "  (default UA)"}`)].join("\n");
    }
    const r = await service.device({ name: a.name });
    return `Device preset applied: ${r.device} (${r.width}x${r.height})\nUser-Agent: ${r.ua}`;
  } },
  devtools: { usage: "[--action]", descKey: "cli.cmd.devtools", args: [{ flag: "action" }], run: async (a) => { const r = await service.devtools({ action: a.action || "toggle" }); return r.open ? "Devtools opened" : "Devtools closed"; } },
  reload: { usage: "", descKey: "cli.cmd.reload", run: async () => { const r = await service.reload(); return `Page reloaded: ${r.url}`; } },
  open_window: { usage: "", descKey: "cli.cmd.open_window", run: async () => { await openWindow(); return "Browser window opened"; } },
  close: { usage: "", descKey: "cli.cmd.close", run: async () => { if (!isRunning()) return "Browser already closed"; await service.close(); await stopService(); return "Browser closed"; } },
  show_notification: { usage: "--message [--type]", descKey: "cli.cmd.show_notification", args: [{ flag: "message" }, { flag: "type" }], run: async (a) => { await service.notify(a); return "Notification shown"; } },
  scroll: { usage: "[--direction --amount]", descKey: "cli.cmd.scroll", args: [{ flag: "direction" }, { flag: "amount" }], run: async (a) => { await service.scroll(a); return `Scrolled ${a.direction || "down"} by ${a.amount || 300}px`; } },
  wait_for_selector: { usage: "--selector [--timeout]", descKey: "cli.cmd.wait_for_selector", args: [{ flag: "selector" }, { flag: "timeout" }], run: async (a) => { await service.waitForSelector(a); return "Element appeared"; } },
  click_and_switch_tab: { usage: "--selector", descKey: "cli.cmd.click_and_switch_tab", args: [{ flag: "selector" }], run: async (a) => { const r = await service.clickSwitchTab(a); return `Clicked ${a.selector}, current URL: ${r.url || ""}`; } },
  iframe_click: { usage: "--iframeSelector --selector", descKey: "cli.cmd.iframe_click", args: [{ flag: "iframeSelector" }, { flag: "selector" }], run: async (a) => { await service.iframeClick(a); return "Clicked in iframe"; } },
  iframe_fill: { usage: "--iframeSelector --selector --value", descKey: "cli.cmd.iframe_fill", args: [{ flag: "iframeSelector" }, { flag: "selector" }, { flag: "value" }], run: async (a) => { await service.iframeFill(a); return "Filled in iframe"; } },
  save_as_pdf: { usage: "", descKey: "cli.cmd.save_as_pdf", run: async () => { await service.pdf(); return "PDF saved"; } },
  get_browser_status: { usage: "", descKey: "cli.cmd.get_browser_status", run: async () => {
    const s = await service.status();
    if (s.installing && Object.keys(s.installing).length > 0) return `Installing ${Object.entries(s.installing).map(([b, p]) => `${b} (${p})`).join(", ")}.`;
    if (!s.open) return "Browser is not open. Use command=open_window or navigate.";
    return `Browser is open\nTitle: ${s.title}\nURL: ${s.url}\nTabs: ${s.tabs}`;
  } },
  list_tabs: { usage: "", descKey: "cli.cmd.list_tabs", run: async () => { const res = await service.tabs(); const list = res?.tabs || []; if (!list.length) return "(no tabs)"; return `Tabs (${list.length}):\n${list.map((p: any, i: number) => `[${i}] ${p.url || p.title}`).join("\n")}`; } },
  switch_tab: { usage: "--index", descKey: "cli.cmd.switch_tab", args: [{ flag: "index" }], run: async (a) => { const r = await service.switchTab(a); return `Switched to tab #${a.index}: ${r.url}`; } },
  new_tab: { usage: "--url", descKey: "cli.cmd.new_tab", args: [{ flag: "url" }], run: async (a) => { const r = await service.newTab(a); return `New tab opened: ${r.url}`; } },
  close_tab: { usage: "[--index]", descKey: "cli.cmd.close_tab", args: [{ flag: "index" }], run: async (a) => { await service.closeTab(a); return a.index !== undefined ? `Closed tab #${a.index}` : "Closed current tab"; } },
  get_element_state: { usage: "--selector", descKey: "cli.cmd.get_element_state", args: [{ flag: "selector" }], run: async (a) => { const r: any = await service.elementState(a); if (!r || !r.exists) return `Element not found: ${a.selector}`; return `Element <${r.tag}>: ${a.selector}\nVisible: ${r.visible}${r.text ? `\nText: ${r.text}` : ""}\nRect: ${r.rect.x},${r.rect.y} ${r.rect.w}x${r.rect.h}`; } },
  scroll_to_element: { usage: "--selector", descKey: "cli.cmd.scroll_to_element", args: [{ flag: "selector" }], run: async (a) => { await service.scrollToElement(a); return "Scrolled to element"; } },
  get_dropdown_options: { usage: "--selector", descKey: "cli.cmd.get_dropdown_options", args: [{ flag: "selector" }], run: async (a) => { const r: any = await service.dropdownOptions(a); if (!r) return `Select not found: ${a.selector}`; return r.map((o: any) => `${o.selected ? "* " : "  "}${o.value}: ${o.text}`).join("\n"); } },
  custom_user_agent: { usage: "--userAgent", descKey: "cli.cmd.custom_user_agent", args: [{ flag: "userAgent" }], run: async (a) => { await service.userAgent(a); return "User-Agent set"; } },
  expect_response: { usage: "--url", descKey: "cli.cmd.expect_response", args: [{ flag: "url" }], run: async (a) => { await service.expectResponse({ urlPattern: a.url }); return `Now expecting response matching: ${a.url}. Use command=assert_response to check.`; } },
  assert_response: { usage: "--id", descKey: "cli.cmd.assert_response", args: [{ flag: "id" }], run: async (a) => { const r = await service.assertResponse({ id: a.id }); return r.matched ? `Response matched: ${r.url} (${r.status})` : r.error || "No response yet"; } },
  get_accessibility_tree: { usage: "[--selector --maxDepth]", descKey: "cli.cmd.get_accessibility_tree", args: [{ flag: "selector" }, { flag: "maxDepth" }], run: async (a) => { const s = await service.accessibility(a); return s ? formatNode(s, 0, a.maxDepth ?? 8) : "(no accessibility info)"; } },
  list_records: { usage: "", descKey: "cli.cmd.list_records", run: async () => { try { const records = await service.annotateRecords(); if (!records || !records.length) return "(no records)"; return records.map((r: any) => { const rect = r.rect ? ` rect=(${r.rect[0]},${r.rect[1]},${r.rect[2]},${r.rect[3]})` : ""; return `[${r.index}] ${r.selector}${rect}${r.note ? ` note="${r.note}"` : ""}`; }).join("\n"); } catch { return "(no records)"; } } },
  read_record_content: { usage: "--id", descKey: "cli.cmd.read_record_content", args: [{ flag: "id" }], run: async (a) => { try { const records = await service.annotateRecords(); const rec = (records || []).find((r: any) => r.index === a.id); if (!rec) return `(record #${a.id} not found)`; return `#${rec.index} ${rec.selector}\nrect: ${JSON.stringify(rec.rect)}\nnote: ${rec.note || "(none)"}`; } catch { return "(no records)"; } } },
  fake_audio: { usage: "--kind [--data --freq --durMs --notes --digits --loop]", descKey: "cli.cmd.fake_audio", args: [{ flag: "kind" }, { flag: "data" }, { flag: "freq" }, { flag: "durMs" }, { flag: "notes" }, { flag: "digits" }, { flag: "loop", type: "boolean" }], run: async (a) => { const r = await service.mediaAudio(a); return `Fake mic audio: ${r.injected}`; } },
  fake_video: { usage: "--kind [--data --url --loop]", descKey: "cli.cmd.fake_video", args: [{ flag: "kind" }, { flag: "data" }, { flag: "url" }, { flag: "loop", type: "boolean" }], run: async (a) => { const r = await service.mediaVideo(a); return `Fake camera video: ${r.injected}`; } },
  profile: { usage: "[--set <name> | --delete <name>]", descKey: "cli.cmd.profile", args: [{ flag: "set" }, { flag: "delete" }], run: async (a) => {
    mkdirSync(getProfilesDir(), { recursive: true });
    const active = getActiveProfile();
    if (a.delete) {
      const name = String(a.delete);
      if (name === "default") return "Cannot delete the default profile";
      if (name === active) return "Cannot delete the active profile; switch to another first (profile --set <name>)";
      const dir = getProfileDir(name);
      if (!existsSync(dir)) return `Profile not found: ${name}`;
      rmSync(dir, { recursive: true, force: true });
      return `Deleted profile: ${name}`;
    }
    if (a.set) {
      const name = String(a.set);
      // 目标配置目录不存在则创建(允许 "profile --set <新名>" 直接新建并切换)
      mkdirSync(getProfileDir(name), { recursive: true });
      setActiveProfile(name);
      setUserDataDir(getProfileDir(name));
      // 切换配置:重启服务使新 user-data-dir 生效(丢当前标签可接受)
      await stopService();
      await ensureService();
      return `Switched to profile: ${name} (browser restarted)`;
    }
    // 列出 user-data 下的配置目录
    const dirs = readdirSync(getProfilesDir(), { withFileTypes: true })
      .filter((d) => d.isDirectory())
      .map((d) => d.name);
    const list = dirs.map((d) => `${d === active ? "*" : " "} ${d}`).join("\n") || "(no profiles yet)";
    return `Profiles:\n${list}\n\nActive: ${active}\nSwitch: profile --set <name> (restarts browser, current tabs lost)\nDelete: profile --delete <name> (not default/active)`;
  } },
  exports: { usage: "", descKey: "cli.cmd.exports", run: async () => {
    const dir = getExportsDir();
    mkdirSync(dir, { recursive: true });
    return `Export directory: ${dir}\n\nHow to export from Edge/Chrome:\n  Passwords: Settings → Profiles → Passwords → ⋯ → Export passwords (CSV)\n  Bookmarks: Settings → Bookmarks → Export (HTML)\nPlace the exported files in the directory above, then the model can read them.\n\nRead passwords without leaking into context:\n  lookup --csv <file.csv> --url <site>\n  fill --selector <input> --value @file:<path>`;
  } },
  lookup: { usage: "--csv <path> --url <site>", descKey: "cli.cmd.lookup", args: [{ flag: "csv" }, { flag: "url" }], run: async (a) => {
    const csv = a.csv;
    const url = a.url;
    if (!csv || !url) return "Usage: lookup --csv <path> --url <site>";
    // 解析密码 CSV(name,url,username,password[,note]),按 url 子串匹配
    const lines = readFileSync(csv, "utf-8").split(/\r?\n/).filter((l) => l.trim());
    for (const line of lines.slice(1)) {
      const cols = splitCsvLine(line);
      if (cols.length < 4) continue;
      const rowUrl = cols[1] || "";
      if (rowUrl.toLowerCase().includes(String(url).toLowerCase())) {
        const username = cols[2] || "";
        const password = cols[3] || "";
        // 密码写入临时文件,返回 @file: 引用(不进上下文);用完即删
        const tmpDir = getTmpDir();
        mkdirSync(tmpDir, { recursive: true });
        const tmp = resolve(tmpDir, `secret-${Date.now()}-${Math.random().toString(36).slice(2, 8)}.tmp`);
        writeFileSync(tmp, password, "utf-8");
        return `Found: ${username} @ ${rowUrl}\nUse: fill --selector <input> --value @file:${tmp}\n(password in temp file, deleted after use)`;
      }
    }
    return `No entry found for: ${url}`;
  } },
};

/** 帮助全文(command=help 或未知命令时返回;命令描述与结构按 toolLang 本地化) */
function buildHelp(): string {
  const lines = Object.entries(COMMANDS).map(
    ([k, v]) => `  ${k.padEnd(22)} ${v.usage.padEnd(42)} ${_t(v.descKey)}`,
  );
  const examples = [_t("cli.example.1"), _t("cli.example.2"), _t("cli.example.3"), _t("cli.example.4")]
    .map((e) => `  ${e}`)
    .join("\n");
  return `${_t("cli.header")}\n\n${_t("cli.usage")}\n\n${_t("cli.commands")}\n${lines.join("\n")}\n\n${_t("cli.examples")}\n${examples}`;
}

/** 描述内嵌的紧凑命令索引(模型无需先调 help 即可用) */
const DESC_INDEX = Object.entries(COMMANDS).map(([k, v]) => `${k} ${v.usage}`).join(" | ");

/** 解析 --flag 风格参数(基于 node:util.parseArgs,未知 flag 报错;tokens 已由 string-argv 正确分词) */
function parseFlagArgs(tokens: string[], def: CmdDef): Record<string, any> {
  if (tokens.length === 0) return {};
  const options: Record<string, any> = {};
  for (const arg of def.args || []) options[arg.flag] = { type: arg.type || "string" };
  const { values } = parseArgs({ args: tokens, options, allowPositionals: true });
  return values;
}

/** 简单 CSV 行解析(处理引号包裹的字段,Edge/Chrome 密码导出格式) */
function splitCsvLine(line: string): string[] {
  const out: string[] = [];
  let cur = "";
  let inQ = false;
  for (let i = 0; i < line.length; i++) {
    const ch = line[i];
    if (inQ) {
      if (ch === '"') {
        if (line[i + 1] === '"') { cur += '"'; i++; } else inQ = false;
      } else cur += ch;
    } else if (ch === '"') inQ = true;
    else if (ch === ",") { out.push(cur); cur = ""; }
    else cur += ch;
  }
  out.push(cur);
  return out;
}

export const opencodeBrowserTool: Plugin = async ({ client, worktree }) => {
  registerLocale("en", en);
  registerLocale("zh", zh);
  loadConfig(worktree);
  const config = getConfig();

  try {
    log.loaded();
    // 懒启动:仅保存启动参数,不立即弹窗;open_window 或首次调工具时启动
    configureService({
      nodePath: config.nodePath || "",
      browsersPath: getBrowsersDir(),
      sessionIsolation: config.sessionIsolation,
      browserType: config.browserType,
      // 多用户配置:默认用当前激活配置的独立 WebView2 用户数据目录
      userDataDir: getProfileDir(getActiveProfile()),
    });

    // 轮询批注发送队列(面板"发送全部" → 推送到对话)
    void startAnnotatePoller(client, log);

    return {
      event: async ({ event }) => {
        if (event.type === "session.deleted") {
          const sid = (event as any).data?.sessionID;
          if (sid) { try { await service.closeSession({ _sessionId: sid }); } catch {} }
        }
      },
      tool: filterDisabled(
        {
          bt_cli: tool({
            description: _t("tool.cli.desc") + "\n" + DESC_INDEX,
            args: {
              // 单字符串命令行: "<command> [--flag value ...]", 如 "navigate --url https://..." / "help"
              args: tool.schema.string().optional().describe(_t("tool.cli.arg.args")),
            },
            async execute(a: any, context: any) {
              // 单字符串命令行: "<command> [--flag value ...]",如 "navigate --url https://..."
              const raw = (typeof a?.args === "string" ? a.args : "").trim();
              if (!raw || raw === "help" || raw === "--help" || raw === "-h") return buildHelp();
              // string-argv 正确分词(处理引号/转义),命令名取第一个 token
              const tokens = stringArgv(raw);
              const cmdName = tokens.shift();
              if (!cmdName) return buildHelp();
              const def = COMMANDS[cmdName];
              if (!def) return `${_t("cli.unknown").replace("{cmd}", cmdName)}\n\n${buildHelp()}`;
              // 剩余 token 交给 parseArgs 解析(未知 flag 会抛错,返回用法提示自愈)
              let params: Record<string, any> = {};
              if (tokens.length > 0) {
                try {
                  params = parseFlagArgs(tokens, def);
                } catch (e: any) {
                  return `${_t("cli.invalid_args").replace("{cmd}", cmdName).replace("{msg}", e.message)}\n\nUsage: bt_cli({ args: "${cmdName} ${def.usage}" })`;
                }
              }
              // @file: 引用解析:值以 @file:<path> 开头时读文件内容作为实际值
              // (密码等敏感值不进 LLM 上下文,如 fill --value @file:C:\...\secret.tmp)
              // 仅删除临时目录(getTmpDir)内的文件,避免误删用户文件
              const tmpDir = resolve(getTmpDir());
              for (const key of Object.keys(params)) {
                const v = params[key];
                if (typeof v === "string" && v.startsWith("@file:")) {
                  const p = v.slice(6);
                  try {
                    params[key] = readFileSync(p, "utf-8").replace(/\r?\n$/, "");
                    // 用完即删(仅限 lookup 生成的临时密码文件)
                    if (resolve(p).startsWith(tmpDir)) {
                      unlinkSync(p);
                    }
                  } catch (e: any) {
                    return `Cannot read @file: ${p}: ${e.message}`;
                  }
                }
              }
              // 透传会话 ID(会话隔离用)
              params._sessionId = context?.sessionID || "";
              try {
                return await def.run(params);
              } catch (e: any) {
                return `${_t("cli.failed").replace("{cmd}", cmdName).replace("{msg}", e.message || e)}`;
              }
            },
          }),
          // 切真实设备属敏感操作,单独保留工具以维持 ask 权限
          bt_set_media_mode: tool({
            description: _t("tool.set_media_mode.desc"),
            args: {
              mode: tool.schema.enum(["simulate", "real"]).optional().describe(_t("tool.set_media_mode.arg.mode")),
            },
            async execute(a: any, context: any) {
              const r = await service.mediaMode({ ...a, _sessionId: context?.sessionID || "" });
              return `Media mode: ${r.mode}`;
            },
          }),
        },
        config.disabledTools,
      ),
    };
  } catch (err) {
    log.error("Init failed", err as Error);
    throw err;
  }
};

function formatNode(node: any, depth: number, maxDepth: number): string {
  if (depth > maxDepth) return "";
  const indent = "│ ".repeat(depth);
  const parts = [indent + node.role];
  if (node.name) parts.push(`"${node.name}"`);
  if (node.value !== undefined && node.value !== "") parts.push(`= ${node.value}`);
  if (node.description) parts.push(`(${node.description})`);
  const line = parts.join(" ");
  if (node.children && node.children.length > 0) {
    const children = node.children
      .map((c: any) => formatNode(c, depth + 1, maxDepth))
      .filter(Boolean)
      .join("\n");
    return children ? line + "\n" + children : line;
  }
  return line;
}

function filterDisabled(tools: Record<string, any>, disabled?: string[]): Record<string, any> {
  if (!disabled || disabled.length === 0) return tools;
  const result: Record<string, any> = {};
  for (const [name, def] of Object.entries(tools)) {
    if (!disabled.includes(name)) result[name] = def;
  }
  return result;
}

/** 轮询批注发送队列:面板"发送全部"/截图"发送"后,拉取记录推送当前对话
 * 注意:插件函数会随会话/切目录多次执行,轮询器必须进程级单例,
 * 否则同一进程会累积多个 interval 空转(server 与 TUI 进程各一份,互不共享)
 */
let annotateTimer: any = null;
let annotateClient: any = null;

function startAnnotatePoller(client: any, log: any): void {
  // 更新 client 引用(插件重载后使用最新 client),但只启动一个轮询器
  annotateClient = client;
  if (annotateTimer) return;
  const POLL_MS = 2000;
  const timer = setInterval(async () => {
    // 服务未启动/已关闭:跳过本次轮询(窗口打开后再恢复推送)
    if (!isRunning()) return;
    const c = annotateClient;
    if (!c) return;
    try {
      const r = await service.annotateConsumeSent();
      const records = r?.records || [];
      if (records.length === 0) return;
      const lines = ["User annotated via panel:", ""];
      const parts: any[] = [];
      for (const item of records) {
        // 截图记录:附带图片 part(裸 base64),AI 可直接看图
        if (item.type === "screenshot" && item.image) {
          parts.push({ type: "file", mime: "image/png", data: item.image, url: "" });
        }
        const label =
          item.type === "screenshot"
            ? `[#${item.index}] 截图${item.url ? ` @ ${item.url}` : ""}`
            : `[#${item.index}] ${item.selector || "(no selector)"}${item.url ? ` @ ${item.url}` : ""}`;
        lines.push(`- ${label}${item.note ? `: ${item.note}` : ""}`);
        lines.push("");
      }
      const text = lines.join("\n");
      const bodyParts: any[] = [{ type: "text", text }];
      for (const p of parts) bodyParts.push(p);
      const sessions = await c.session.list();
      if (sessions?.data?.length) {
        await c.session.prompt({
          path: { id: sessions.data[0].id },
          body: { noReply: false, parts: bodyParts },
        });
      }
    } catch {
      // 服务未就绪/已退出,停止轮询
      clearInterval(timer);
      annotateTimer = null;
    }
  }, POLL_MS);
  // 不阻塞进程退出
  if (typeof (timer as any).unref === "function") (timer as any).unref();
  annotateTimer = timer;
}

// Export default for npm loader
export default opencodeBrowserTool;
