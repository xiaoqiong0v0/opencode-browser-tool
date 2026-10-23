import { readFileSync, writeFileSync, readdirSync, unlinkSync, mkdirSync, rmSync, existsSync } from "fs";
import { resolve } from "path";
import stringArgv from "string-argv";
import { parseArgs } from "node:util";
import {
  loadConfig, getConfig, getBrowsersDir, getCacheDir, getProfilesDir,
  getExportsDir, getTmpDir, getActiveProfile, setActiveProfile, getSessionDataDir, getSessionTmpDir,
  getSessionsDir, resolveProfilePath, isInsideDir,
} from "./config/index.js";
import { registerLocale, tf } from "./i18n/index.js";
import en from "./i18n/en.js";
import zh from "./i18n/zh.js";
import {
  configureService, setAppDirectory, setServiceReadyHandler, setSessionIdResolver, setTitleResolver,
  resolveSessionId, openWindow, isRunning, isStarting, closeSession, closeAllSessions, clearClosedState, service, probeStatus,
  clearAttachNotifyForSid,
} from "./client.js";
import { startNotifyServer } from "./notify-server.js";
import { startDownload, getBinaryStatus, describeBinaryStatus } from "./binary.js";
import { sanitizeForModel } from "./util/sanitize.js";
import { tool, type Plugin } from "@opencode-ai/plugin";
import { log } from "./logger.js";
import { createSessionIDCache, extractSessionIDFromEvent } from "./session-id.js";

/** 取 i18n 文案(tf 的快捷别名;vars 可选,替换 {name} 占位符) */
const _t = (k: string, vars?: Record<string, string | number>) => tf(k, vars);

/** 当前会话 id(工具调用的显式 _sessionId 优先,否则用钩子缓存兜底) */
function sidOf(a: any): string {
  return resolveSessionId(typeof a?._sessionId === "string" ? a._sessionId : "");
}

/** 构造只带会话 id 的请求体(用于无参但需按会话路由的命令,如 list_tabs / reload) */
function sidArgs(a: any): { _sessionId: string } {
  return { _sessionId: sidOf(a) };
}

/** 降级说明:服务端在无可信通道、退回 untrusted DOM/JS 时返回 degraded,统一在此追加标注 */
function degradedNote(r: any): string {
  return r && r.degraded ? _t("cli.note.degraded", { reason: r.degradedReason || "untrusted fallback" }) : "";
}

/** 就绪说明:导航/前进后退未在超时内就绪时如实标注(不假装成功) */
function readyNote(r: any): string {
  if (!r || r.ready !== false) return "";
  const rs = r.readyState ? `, readyState=${r.readyState}` : "";
  return _t("cli.note.not_ready", { ms: r.waitedMs ?? "?", rs });
}

/** 滚动稳定说明:未在超时内稳定时如实标注 */
function stableNote(r: any): string {
  if (!r || r.stable !== false) return "";
  return _t("cli.note.scroll_unstable", { ms: r.waitedMs ?? "?" });
}

/** scroll_to_element 到达说明:未进入视口时如实标注(含原因) */
function reachedNote(r: any): string {
  if (!r || r.reached !== false) return "";
  const why = r.reason ? `: ${r.reason}` : "";
  return _t("cli.note.reached", {
    where: r.inViewport ? "in viewport" : "not in viewport",
    ms: r.waitedMs ?? "?",
    why,
  });
}

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

/** `--timeout` 合法区间(ms):与 Rust `service.rs` 的 `TIMEOUT_MIN_MS`/`TIMEOUT_MAX_MS` 同口径 */
const TIMEOUT_MIN_MS = 100;
const TIMEOUT_MAX_MS = 60000;

/** 解析并规范化模型传入的 `--timeout`(毫秒,与 `wait_for_selector` 既有单位一致)
 * - 未传/空:返回空串,各命令走服务端默认(1.9.0 批次4 起 eval/CDP/截图/wait_for_selector 均 15s)
 * - 非正整数:抛错 —— 快速明确拒绝,不带着坏值进浏览器
 * - 超出 [100, 60000]:钳制并把 `a.timeout` 改写为钳后值(实际生效值),返回如实标注文案
 * 服务端 Rust 侧同口径二次钳制兜底(直连 HTTP 也安全);响应体的 timeoutClamped 与这里一致
 */
function timeoutNote(a: any): string {
  const raw = a?.timeout;
  if (raw === undefined || raw === null || raw === "") return "";
  const n = typeof raw === "number" ? raw : Number(String(raw).trim());
  if (!Number.isInteger(n) || n <= 0) {
    throw new Error(_t("cli.err.timeout_invalid", { v: String(raw) }));
  }
  const c = Math.min(Math.max(n, TIMEOUT_MIN_MS), TIMEOUT_MAX_MS);
  a.timeout = c;
  return c === n ? "" : _t("cli.note.timeout_clamped", { ms: c });
}

const COMMANDS: Record<string, CmdDef> = {
  navigate: { usage: "--url", descKey: "cli.cmd.navigate", args: [{ flag: "url" }], run: async (a) => {
    const r = await service.navigate(a);
    if (r.installError) return _t("cli.out.navigate.install_failed", { error: r.error });
    return r.installing ? _t("cli.out.navigate.installing", { name: r.installing, progress: r.progress }) : _t("cli.out.navigate.ok", { url: r.url }) + readyNote(r);
  } },
  click: { usage: "--selector [--timeout ms]", descKey: "cli.cmd.click", args: [{ flag: "selector" }, { flag: "timeout" }], run: async (a) => { const tn = timeoutNote(a); const r = await service.click(a); return _t("cli.out.click") + degradedNote(r) + tn; } },
  fill: { usage: "--selector --value [--timeout ms]", descKey: "cli.cmd.fill", args: [{ flag: "selector" }, { flag: "value" }, { flag: "timeout" }], run: async (a) => { const tn = timeoutNote(a); const r = await service.fill(a); return _t("cli.out.fill") + degradedNote(r) + tn; } },
  clear: { usage: "--selector [--timeout ms]", descKey: "cli.cmd.clear", args: [{ flag: "selector" }, { flag: "timeout" }], run: async (a) => { const tn = timeoutNote(a); const r = await service.clear(a); return _t("cli.out.clear") + degradedNote(r) + tn; } },
  select: { usage: "--selector --value [--timeout ms]", descKey: "cli.cmd.select", args: [{ flag: "selector" }, { flag: "value" }, { flag: "timeout" }], run: async (a) => { const tn = timeoutNote(a); const r = await service.select(a); return _t("cli.out.select") + degradedNote(r) + tn; } },
  hover: { usage: "--selector [--timeout ms]", descKey: "cli.cmd.hover", args: [{ flag: "selector" }, { flag: "timeout" }], run: async (a) => { const tn = timeoutNote(a); const r = await service.hover(a); return _t("cli.out.hover") + degradedNote(r) + tn; } },
  drag: { usage: "--sourceSelector --targetSelector [--timeout ms]", descKey: "cli.cmd.drag", args: [{ flag: "sourceSelector" }, { flag: "targetSelector" }, { flag: "timeout" }], run: async (a) => { const tn = timeoutNote(a); await service.drag(a); return _t("cli.out.drag") + tn; } },
  press_key: { usage: "--key [--selector] [--timeout ms]", descKey: "cli.cmd.press_key", args: [{ flag: "key" }, { flag: "selector" }, { flag: "timeout" }], run: async (a) => { const tn = timeoutNote(a); await service.pressKey(a); return _t("cli.out.press_key", { key: a.key }) + tn; } },
  upload_file: { usage: "--selector --filePath [--timeout ms]", descKey: "cli.cmd.upload_file", args: [{ flag: "selector" }, { flag: "filePath" }, { flag: "timeout" }], run: async (a) => { const tn = timeoutNote(a); await service.uploadFile(a); return _t("cli.out.upload_file") + tn; } },
  screenshot: { usage: "[--selector] [--timeout ms]", descKey: "cli.cmd.screenshot", args: [{ flag: "selector" }, { flag: "timeout" }], run: async (a) => {
    const tn = timeoutNote(a);
    const r = await service.screenshot(a);
    // 元素截图:先滚动到可见再截;rect 超出视口时走越界路径(截全),滚动未稳定/未到达如实标注
    const where = a.selector ? _t("cli.out.screenshot.element", { sel: a.selector }) : _t("cli.out.screenshot.fullpage");
    const note = (r as any)?.beyondViewport ? _t("cli.note.beyond") : "";
    return { output: _t("cli.out.screenshot", { where }) + note + reachedNote(r) + stableNote(r) + tn, attachments: [{ type: "file", mime: r.mime || "image/png", url: `data:${r.mime || "image/png"};base64,${r.base64}` }] };
  } },
  // evaluate:超时最常用(脚本 await 慢操作);超时返回明确 eval timeout 错误,不再恒 5s
  evaluate: { usage: "--script [--timeout ms]", descKey: "cli.cmd.evaluate", args: [{ flag: "script" }, { flag: "timeout" }], run: async (a) => { const tn = timeoutNote(a); return JSON.stringify(await service.evaluate(a), null, 2) + tn; } },
  get_visible_text: { usage: "[--selector]", descKey: "cli.cmd.get_visible_text", args: [{ flag: "selector" }], run: async (a) => (await service.visibleText(a))?.text || _t("cli.out.no_visible_text") },
  get_visible_html: { usage: "[--selector --removeScripts --removeComments --maxLength]", descKey: "cli.cmd.get_visible_html", args: [{ flag: "selector" }, { flag: "removeScripts", type: "boolean" }, { flag: "removeComments", type: "boolean" }, { flag: "maxLength" }], run: async (a) => {
    // 宿主要求 tool 返回字符串;服务端返回对象 {html,truncated},这里取出 html 文本本体
    const r = await service.visibleHtml(a);
    const html = r?.html || "";
    if (!html) return _t("cli.out.no_visible_html");
    // 被截断时追加提示:html.length 即服务端实际生效的 maxLength(已 slice),无需再传参
    return r?.truncated ? `${html}\n${_t("cli.out.html_truncated", { n: html.length })}` : html;
  } },
  console_logs: { usage: "[--type --search --limit --clear]", descKey: "cli.cmd.console_logs", args: [{ flag: "type" }, { flag: "search" }, { flag: "limit" }, { flag: "clear", type: "boolean" }], run: async (a) => { const r = await service.consoleLogs(a); return (r.logs || []).join("\n") || _t("cli.out.no_logs"); } },
  go_back: { usage: "", descKey: "cli.cmd.go_back", run: async () => { const r = await service.goBack(); return _t("cli.out.go_back", { url: r.url }) + readyNote(r); } },
  go_forward: { usage: "", descKey: "cli.cmd.go_forward", run: async () => { const r = await service.goForward(); return _t("cli.out.go_forward", { url: r.url }) + readyNote(r); } },
  // resize:宽高=内容区(视口)尺寸;被钳到下限时如实标注(服务端返回 clamped/content/window)
  resize: { usage: "--width --height", descKey: "cli.cmd.resize", args: [{ flag: "width" }, { flag: "height" }], run: async (a) => {
    const r = await service.resize(a);
    const c = r.content || {};
    return _t("cli.out.resize", { w: c.w, h: c.h }) + (r.clamped ? _t("cli.out.resize_clamped") : "");
  } },
  set_device: { usage: "[--name]", descKey: "cli.cmd.set_device", args: [{ flag: "name" }], run: async (a) => {
    if (!a.name) {
      const list = await service.deviceList(sidArgs(a));
      return [_t("cli.out.device_list_header"), "", ...(list.devices || []).map((d: any) => `  ${d.name}  ${d.width}x${d.height}${d.ua ? `  ${d.ua.slice(0, 60)}...` : "  (default UA)"}`)].join("\n");
    }
    const r = await service.device({ name: a.name });
    return _t("cli.out.device_applied", { name: r.device, w: r.width, h: r.height, ua: r.ua });
  } },
  devtools: { usage: "[--action]", descKey: "cli.cmd.devtools", args: [{ flag: "action" }], run: async (a) => {
    const r = await service.devtools({ action: a.action || "toggle" });
    // supported=false:该平台无法真正关闭(Windows/wry 空实现)→ 如实提示,不接受误导性的「已关闭」
    if (r?.supported === false) return _t("cli.out.devtools_unsupported");
    return r.open ? _t("cli.out.devtools_open") : _t("cli.out.devtools_closed");
  } },
  reload: { usage: "", descKey: "cli.cmd.reload", run: async (a) => { const r = await service.reload(sidArgs(a)); return r?.url ? _t("cli.out.reload", { url: r.url }) : _t("cli.out.reload_nourl"); } },
  open_window: { usage: "", descKey: "cli.cmd.open_window", run: async (a) => { await openWindow(sidOf(a)); return _t("cli.out.open_window"); } },
  close: { usage: "", descKey: "cli.cmd.close", run: async (a) => {
    const sid = sidOf(a);
    // 运行中或**启动中**都算「有实例可关」:启动中的实例由 closeSession 置 closedSids,启动链随后自毁
    if (!isRunning(sid) && !isStarting(sid)) return _t("cli.out.close_already");
    await closeSession(sid);
    return _t("cli.out.close_done");
  } },
  show_notification: { usage: "--message [--type]", descKey: "cli.cmd.show_notification", args: [{ flag: "message" }, { flag: "type" }], run: async (a) => { await service.notify(a); return _t("cli.out.notify_shown"); } },
  scroll: { usage: "[--direction --amount]", descKey: "cli.cmd.scroll", args: [{ flag: "direction" }, { flag: "amount" }], run: async (a) => { const r = await service.scroll(a); return _t("cli.out.scroll", { dir: a.direction || "down", n: a.amount || 300 }) + stableNote(r); } },
  wait_for_selector: { usage: "--selector [--timeout ms]", descKey: "cli.cmd.wait_for_selector", args: [{ flag: "selector" }, { flag: "timeout" }], run: async (a) => { const tn = timeoutNote(a); await service.waitForSelector(a); return _t("cli.out.wait_appeared") + tn; } },
  click_and_switch_tab: { usage: "--selector", descKey: "cli.cmd.click_and_switch_tab", args: [{ flag: "selector" }], run: async (a) => { const r = await service.clickSwitchTab(a); return _t("cli.out.click_switch_tab", { sel: a.selector, url: r.url || "" }) + degradedNote(r); } },
  iframe_click: { usage: "--iframeSelector --selector [--timeout ms]", descKey: "cli.cmd.iframe_click", args: [{ flag: "iframeSelector" }, { flag: "selector" }, { flag: "timeout" }], run: async (a) => { const tn = timeoutNote(a); const r = await service.iframeClick(a); return _t("cli.out.iframe_click") + degradedNote(r) + tn; } },
  iframe_fill: { usage: "--iframeSelector --selector --value [--timeout ms]", descKey: "cli.cmd.iframe_fill", args: [{ flag: "iframeSelector" }, { flag: "selector" }, { flag: "value" }, { flag: "timeout" }], run: async (a) => { const tn = timeoutNote(a); const r = await service.iframeFill(a); return _t("cli.out.iframe_fill") + degradedNote(r) + tn; } },
  get_browser_status: { usage: "", descKey: "cli.cmd.get_browser_status", run: async (a) => {
    // 纯查询:不 spawn、不懒启动(未运行只提示,不弹窗)
    const s = await probeStatus(1500, sidOf(a));
    if (!s) {
      const b = getBinaryStatus();
      // bt-shell 状态文案由 describeBinaryStatus 提供(已保证不含本机绝对路径)
      return _t("cli.out.status.not_running", { bin: describeBinaryStatus(b) });
    }
    if (s.installing && Object.keys(s.installing).length > 0) return _t("cli.out.status.installing", { list: Object.entries(s.installing).map(([b, p]) => `${b} (${p})`).join(", ") });
    if (!s.open) return _t("cli.out.status.not_open");
    return _t("cli.out.status.open", { title: s.title, url: s.url, n: s.tabs });
  } },
  list_tabs: { usage: "", descKey: "cli.cmd.list_tabs", run: async (a) => { const res = await service.tabs(sidArgs(a)); const list = res?.tabs || []; if (!list.length) return _t("cli.out.tabs_none"); return `${_t("cli.out.tabs_header", { n: list.length })}\n${list.map((p: any, i: number) => `[${i}] ${p.url || p.title}`).join("\n")}`; } },
  switch_tab: { usage: "--index", descKey: "cli.cmd.switch_tab", args: [{ flag: "index" }], run: async (a) => { const r = await service.switchTab(a); return _t("cli.out.switch_tab", { i: a.index, url: r.url }); } },
  new_tab: { usage: "--url", descKey: "cli.cmd.new_tab", args: [{ flag: "url" }], run: async (a) => { const r = await service.newTab(a); return _t("cli.out.new_tab", { url: r.url }); } },
  close_tab: { usage: "[--index]", descKey: "cli.cmd.close_tab", args: [{ flag: "index" }], run: async (a) => { await service.closeTab(a); return a.index !== undefined ? _t("cli.out.close_tab", { i: a.index }) : _t("cli.out.close_tab_current"); } },
  get_element_state: { usage: "--selector [--timeout ms]", descKey: "cli.cmd.get_element_state", args: [{ flag: "selector" }, { flag: "timeout" }], run: async (a) => { const tn = timeoutNote(a); const r: any = await service.elementState(a); if (!r || !r.exists) return _t("cli.out.element_not_found", { sel: a.selector }); const text = r.text ? `\nText: ${r.text}` : ""; return _t("cli.out.element_state", { tag: r.tag, sel: a.selector, visible: r.visible, text, rect: `${r.rect.x},${r.rect.y} ${r.rect.w}x${r.rect.h}` }) + tn; } },
  scroll_to_element: { usage: "--selector [--timeout ms]", descKey: "cli.cmd.scroll_to_element", args: [{ flag: "selector" }, { flag: "timeout" }], run: async (a) => { const tn = timeoutNote(a); const r = await service.scrollToElement(a); return _t("cli.out.scroll_to_element") + reachedNote(r) + tn; } },
  get_dropdown_options: { usage: "--selector [--timeout ms]", descKey: "cli.cmd.get_dropdown_options", args: [{ flag: "selector" }, { flag: "timeout" }], run: async (a) => { const tn = timeoutNote(a); const r: any = await service.dropdownOptions(a); const opts: any[] = r?.options ?? []; if (!opts.length) return _t("cli.out.dropdown_none", { sel: a.selector }); return opts.map((o: any) => `${o.selected ? "* " : "  "}${o.value}: ${o.text}`).join("\n") + tn; } },
  custom_user_agent: { usage: "--userAgent", descKey: "cli.cmd.custom_user_agent", args: [{ flag: "userAgent" }], run: async (a) => { await service.userAgent(a); return _t("cli.out.ua_set"); } },
  expect_response: { usage: "--url", descKey: "cli.cmd.expect_response", args: [{ flag: "url" }], run: async (a) => { await service.expectResponse({ urlPattern: a.url }); return _t("cli.out.expect_response", { url: a.url }); } },
  assert_response: { usage: "--id", descKey: "cli.cmd.assert_response", args: [{ flag: "id" }], run: async (a) => { const r = await service.assertResponse({ id: a.id }); return r.matched ? _t("cli.out.assert_matched", { url: r.url, status: r.status }) : r.error || _t("cli.out.assert_none"); } },
  get_accessibility_tree: { usage: "[--selector --maxDepth] [--timeout ms]", descKey: "cli.cmd.get_accessibility_tree", args: [{ flag: "selector" }, { flag: "maxDepth" }, { flag: "timeout" }], run: async (a) => { const tn = timeoutNote(a); const s = await service.accessibility(a); return (s ? formatNode(s, 0, a.maxDepth ?? 8) : _t("cli.out.no_ax")) + tn; } },
  list_records: { usage: "", descKey: "cli.cmd.list_records", run: async (a) => { try { const records = await service.annotateRecords(sidArgs(a)); if (!records || !records.length) return _t("cli.out.no_records"); return records.map((r: any) => { const rect = r.rect ? ` rect=(${r.rect[0]},${r.rect[1]},${r.rect[2]},${r.rect[3]})` : ""; const img = r.image ? " +img" : ""; const sent = r.sent ? " [sent]" : ""; return `[${r.index}] ${r.selector}${rect}${img}${sent}${r.note ? ` note="${r.note}"` : ""}`; }).join("\n"); } catch { return _t("cli.out.no_records"); } } },
  read_record_content: { usage: "--id", descKey: "cli.cmd.read_record_content", args: [{ flag: "id" }], run: async (a) => { try { const records = await service.annotateRecords({ ...sidArgs(a), includeImages: true }); const rec = (records || []).find((r: any) => r.index === a.id); if (!rec) return _t("cli.out.record_not_found", { id: a.id }); let img = ""; if (rec.image) { const p = saveShotImage(log, sidOf(a), rec.index, rec.image); const kb = Math.round(rec.image.length / 1024); img = p ? _t("cli.out.record_image_saved", { kb, path: p }) : _t("cli.out.record_image", { kb }); } return `#${rec.index} ${rec.selector}\nrect: ${JSON.stringify(rec.rect)}${img}\nnote: ${rec.note || _t("cli.out.record_note_none")}`; } catch { return _t("cli.out.no_records"); } } },
  fake_audio: { usage: "--kind [--data --freq --durMs --notes --digits --loop]", descKey: "cli.cmd.fake_audio", args: [{ flag: "kind" }, { flag: "data" }, { flag: "freq" }, { flag: "durMs" }, { flag: "notes" }, { flag: "digits" }, { flag: "loop", type: "boolean" }], run: async (a) => { const r = await service.mediaAudio(a); return _t("cli.out.fake_audio", { injected: r.injected }); } },
  fake_video: { usage: "--kind [--data --url --loop]", descKey: "cli.cmd.fake_video", args: [{ flag: "kind" }, { flag: "data" }, { flag: "url" }, { flag: "loop", type: "boolean" }], run: async (a) => { const r = await service.mediaVideo(a); return _t("cli.out.fake_video", { injected: r.injected }); } },
  profile: { usage: "[--set <name> | --delete <name>]", descKey: "cli.cmd.profile", args: [{ flag: "set" }, { flag: "delete" }], run: async (a) => {
    const active = getActiveProfile();
    /** 解析 profile 路径;名字非法时返回错误文案(不 mkdir、不 rm) */
    const resolveOrError = (raw: any): { dir?: string; name: string; err?: string } => {
      const name = String(raw);
      try {
        return { dir: resolveProfilePath(name), name };
      } catch {
        return { name, err: _t("cli.err.profile_invalid_name", { name }) };
      }
    };
    if (a.delete) {
      const r = resolveOrError(a.delete);
      if (r.err) return r.err;
      const name = r.name;
      if (name === "default") return _t("cli.out.profile_no_delete_default");
      if (name === active) return _t("cli.out.profile_no_delete_active");
      const dir = r.dir!;
      if (!existsSync(dir)) return _t("cli.out.profile_not_found", { name });
      rmSync(dir, { recursive: true, force: true });
      return _t("cli.out.profile_deleted", { name });
    }
    if (a.set) {
      const r = resolveOrError(a.set);
      if (r.err) return r.err;
      const name = r.name;
      // 目标配置目录不存在则创建(允许 "profile --set <新名>" 直接新建并切换)
      mkdirSync(r.dir!, { recursive: true });
      setActiveProfile(name);
      // 1.9.0 会话隔离:profile 作用域改为「影响**后续新会话**的数据根(sessions/<profile>/<sid>)」。
      // **不再重启共享 shell** —— 运行中的会话各自持有独立实例,切换互不打扰(旧实现会杀掉全局唯一窗口)。
      return _t("cli.out.profile_switched", { name });
    }
    // 列出 user-data 下的配置目录
    mkdirSync(getProfilesDir(), { recursive: true });
    const dirs = readdirSync(getProfilesDir(), { withFileTypes: true })
      .filter((d) => d.isDirectory())
      .map((d) => d.name);
    const list = dirs.map((d) => `${d === active ? "*" : " "} ${d}`).join("\n") || _t("cli.out.profiles_none");
    return _t("cli.out.profiles_header", { list, active });
  } },
  exports: { usage: "", descKey: "cli.cmd.exports", run: async () => {
    const dir = getExportsDir();
    mkdirSync(dir, { recursive: true });
    return _t("cli.out.exports", { dir });
  } },
  lookup: { usage: "--csv <path> --url <site>", descKey: "cli.cmd.lookup", args: [{ flag: "csv" }, { flag: "url" }], run: async (a) => {
    const csv = a.csv;
    const url = a.url;
    if (!csv || !url) return _t("cli.out.lookup_usage");
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
        return _t("cli.out.lookup_found", { user: username, url: rowUrl, tmp });
      }
    }
    return _t("cli.out.lookup_none", { url });
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

export const opencodeBrowserTool: Plugin = async ({ client, worktree, directory }) => {
  // 供 resolveShellBinary 定位本地 Rust 构建(publish-local 到缓存目录后仍能找到仓库)
  setAppDirectory(directory || "");
  registerLocale("en", en);
  registerLocale("zh", zh);
  loadConfig(worktree);
  const config = getConfig();

  try {
    log.loaded();
    // 懒启动:仅保存启动参数,不立即弹窗;open_window 或首次调工具时启动
    // 1.9.0 会话隔离:每个会话独立实例(独立端口 + 独立 user-data-dir),由 client.ts 的注册表管理
    configureService({
      nodePath: config.nodePath || "",
      browsersPath: getBrowsersDir(),
      // 会话数据目录:sessions/<profile>/<sid>(profile 影响后续新会话的数据根)
      resolveDataDir: (sid: string) => getSessionDataDir(sid),
    });
    // 会话 id 兜底解析器:工具调用没带 _sessionId 时用钩子缓存
    setSessionIdResolver(() => sessionCache.get());
    // 可选会话标题解析器:client.session.get 之类;拿不到/超时回退 sid 后缀,不阻塞启动
    setTitleResolver(async (sid: string) => {
      try {
        const r = await client.session.get({ path: { id: sid } });
        return r?.data?.title || null;
      } catch {
        return null;
      }
    });

    // 启动即后台下载 bt-shell(fire-and-forget:不阻塞启动,失败不抛出;状态见 getBinaryStatus)
    startDownload();

    // 事件驱动投递:插件本地监听 shell 通知(推送式,替代轮询)
    setupAnnotateDelivery(client);

    return {
      event: async ({ event }) => {
        // 缓存最近会话 id:这是投递批注最可靠的 sessionID 来源
        if (event.type === "session.deleted") {
          // 删除会话(1.9.0 语义升级):清钩子缓存 + **回收该会话自己的浏览器实例**。
          // 仍是「绝不因删除而 spawn」:closeSession 只在注册表里有该 sid 时才动进程,
          // 没有实例就直接返回(不会再走 callApi → ensureInstance 那条会弹窗的老路)。
          const delId = extractSessionIDFromEvent(event);
          if (delId) {
            sessionCache.clearIf(delId);
            await closeSession(delId);
            clearClosedState(delId);
            // 清该 sid 的附着 notify 记录(附着模式共享 shell,避免残留 key 无限增长)
            clearAttachNotifyForSid(delId);
            // 回收该会话的数据目录(P1:否则退出后磁盘目录永不释放)
            removeSessionDataDir(delId);
          }
          return;
        }
        sessionCache.updateFromEvent(event);
      },
      // 用户发消息 / 调工具时刷新当前会话 id(参考项目用 ctx.sessionID,此处为等价钩子来源)
      "chat.message": async (input: any) => {
        sessionCache.updateFromHookInput(input);
      },
      "tool.execute.before": async (input: any) => {
        sessionCache.updateFromHookInput(input);
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
                  // 解析错误可能夹带底层异常文本:进模型前先脱敏(用法提示本身不含路径)
                  return `${_t("cli.invalid_args").replace("{cmd}", cmdName).replace("{msg}", sanitizeForModel(e?.message || String(e)))}\n\nUsage: bt_cli({ args: "${cmdName} ${def.usage}" })`;
                }
              }
              // @file: 引用解析:值以 @file:<path> 开头时读文件内容作为实际值
              // (密码等敏感值不进 LLM 上下文,如 fill --value @file:C:\...\secret.tmp)
              // 安全边界:只允许读全局 tmp(getTmpDir)或**该会话** tmp(getSessionTmpDir)之内的文件;
              // 其余路径一律拒绝(不读取),避免模型借助 @file: 读取任意本地文件。
              // sid 形态校验:resolveSessionId 会原样返回外部传入的 sessionID,任意字符串(含 .. 、路径分隔符)
              // 会把 getSessionTmpDir 带到 sessions 目录之外,从而左右 allowed dirs。不匹配 SESSION_SID_RE
              // 一律回退 "default" 桶(而非 resolveSessionId(""):后者仍可能返回同一个非法的钩子缓存值)。
              // "default" 本身合法,getSessionTmpDir("default") = sessions/<profile>/default/tmp,必在 sessions 内。
              const fileSidRaw = resolveSessionId(context?.sessionID || "");
              const fileSid = SESSION_SID_RE.test(fileSidRaw) ? fileSidRaw : "default";
              const globalTmpDir = getTmpDir();
              const allowedTmpDirs = [globalTmpDir, getSessionTmpDir(fileSid)];
              for (const key of Object.keys(params)) {
                const v = params[key];
                if (typeof v === "string" && v.startsWith("@file:")) {
                  const p = v.slice(6);
                  const abs = resolve(p);
                  if (!allowedTmpDirs.some((d) => isInsideDir(abs, d))) {
                    return _t("cli.err.file_outside_tmp", { path: p });
                  }
                  try {
                    params[key] = readFileSync(abs, "utf-8").replace(/\r?\n$/, "");
                    // 用完即删(仅限 lookup 生成的全局 tmp 临时密码文件;会话 tmp 内的截图等保留)
                    if (isInsideDir(abs, globalTmpDir)) {
                      unlinkSync(abs);
                    }
                  } catch (e: any) {
                    // {path} 是模型自己传入的值,保持原样;{msg} 来自 fs 错误,必须脱敏
                    return _t("cli.read_file_failed", { path: p, msg: sanitizeForModel(e?.message || String(e)) });
                  }
                }
              }
              // 透传会话 ID(会话隔离用),并刷新钩子缓存(兜底:即使钩子未触发也能拿到 sid)
              sessionCache.updateFromHookInput(context);
              setAppDirectory(context?.directory || "");
              params._sessionId = context?.sessionID || "";
              try {
                return await def.run(params);
              } catch (e: any) {
                // 命令失败可能带 fs/网络错误文本:统一脱敏后再回模型
                return `${_t("cli.failed").replace("{cmd}", cmdName).replace("{msg}", sanitizeForModel(e?.message || String(e)))}`;
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
              sessionCache.updateFromHookInput(context);
              setAppDirectory(context?.directory || "");
              const r = await service.mediaMode({ ...a, _sessionId: context?.sessionID || "" });
              return _t("cli.out.media_mode", { mode: r.mode });
            },
          }),
        },
        config.disabledTools,
      ),
      // 插件卸载/进程退出:优雅关闭全部会话实例(按注册表 PID,绝不按名;附着模式不动外部进程)
      dispose: async () => {
        await closeAllSessions();
      },
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

/** 事件驱动投递:shell 面板"发送全部"/截图"发送"后 POST 插件本地 /notify/<sid> 触发 drain
 * 注意:插件函数会随会话/切目录多次执行,notify server 与 drain 状态必须进程级单例,
 * 否则同一进程会累积多个监听/重复投递(server 与 TUI 进程各一份,互不共享)
 */
let annotateClient: any = null;
/** drain 进行中(按 sid 隔离:会话 A 的 drain 不会挡住会话 B 的投递) */
const drainingSid = new Set<string>();
/** 最近一次见到的会话 id 缓存(来自 event/chat.message/tool.execute.before 钩子) */
const sessionCache = createSessionIDCache();

/** 把截图 base64 落盘为临时 PNG,返回路径(失败返回 null);用于发送时保留图片与给出可读路径
 * 1.9.0:落盘到**该会话**的临时目录 sessions/<profile>/<sid>/tmp(不再与其它会话共享全局 tmp) */
function saveShotImage(log: any, sid: string, index: number, b64: string): string | null {
  try {
    const dir = resolve(getSessionTmpDir(sid));
    mkdirSync(dir, { recursive: true });
    const p = resolve(dir, `shot-${Date.now()}-${index}.png`);
    writeFileSync(p, Buffer.from(b64, "base64"));
    return p;
  } catch (e) {
    log.error("save screenshot image failed", e as Error);
    return null;
  }
}

/** 合法会话 id 形态(opencode 生成的 sid);不匹配则不删目录,避免意外路径 */
const SESSION_SID_RE = /^[A-Za-z0-9_-]{1,128}$/;
/** 会话目录删除失败后的重试延迟(ms;Windows 上文件常被短暂占用) */
const SESSION_DIR_RETRY_MS = 800;

/** 同步删除某 sid 的数据目录(失败仅记录,由调用方决定是否重试) */
function rmSessionDir(dir: string): boolean {
  try {
    rmSync(dir, { recursive: true, force: true });
    log.info(`[session] removed data dir for deleted session: ${dir}`);
    return true;
  } catch (e) {
    log.error(`[session] failed to remove data dir ${dir}: ${(e as Error).message}`, e as Error);
    return false;
  }
}

/**
 * 会话删除后回收**所有 profile 下**属于该 sid 的数据目录(sessions/<profile>/<sid>)。
 * 用户切换过 activeProfile 时,老 profile 目录里的同名会话也要清掉,故枚举 sessions 根的直接子目录。
 * 约束:sid 必须合法;每个目标必须在 getSessionsDir() 之内,且不等于 sessions 根或 profile 根
 *      (只删名为 <sid> 的目录,profile 目录本身永不删除)。
 * Windows 文件锁导致失败时 800ms 后对失败列表重试一次;全部成功则不设定时器。
 */
function removeSessionDataDir(sid: string): void {
  if (!SESSION_SID_RE.test(sid)) {
    log.error(`[session] skip data dir cleanup: invalid session id (${sid.length} chars)`);
    return;
  }
  const sessionsDir: string = getSessionsDir();
  // 枚举直接子目录:每个 = 一个 profile;读不到(根不存在/无权限)视为无可回收目录
  let profileNames: string[];
  try {
    profileNames = readdirSync(sessionsDir, { withFileTypes: true })
      .filter((e) => e.isDirectory())
      .map((e) => e.name);
  } catch {
    return;
  }
  const targets: string[] = [];
  for (const name of profileNames) {
    const profileDir: string = resolve(sessionsDir, name);
    const target: string = resolve(profileDir, sid);
    // 路径断言:仍在 sessions 根内,且不是 sessions 根/profile 根本身
    if (!isInsideDir(target, sessionsDir) || target === sessionsDir || target === profileDir) continue;
    if (!existsSync(target)) continue;
    targets.push(target);
  }
  if (!targets.length) return;
  const failed: string[] = targets.filter((t) => !rmSessionDir(t));
  if (!failed.length) {
    log.info(`[session] cleaned data dir for sid=${sid} across ${targets.length} profile dir(s)`);
    return;
  }
  const timer = setTimeout(() => {
    const stillFailing: string[] = failed.filter((t) => !rmSessionDir(t));
    if (stillFailing.length) {
      log.error(`[session] data dir cleanup still failing after retry (sid=${sid}): ${stillFailing.length} dir(s)`);
    }
  }, SESSION_DIR_RETRY_MS);
  // 不让重试定时器拖住进程退出
  (timer as any).unref?.();
}

/** 事件驱动投递:插件本地监听 shell 通知(收到即 drain),不再 setInterval 轮询 */
function setupAnnotateDelivery(client: any): void {
  // 更新 client 引用(插件重载后使用最新 client)
  annotateClient = client;
  // 服务就绪时做一次 drain(覆盖"通知机制建立前 shell 已发送过"的窗口,幂等;含 shell 重启)
  // 1.9.0:就绪回调带 sid → 只 drain 该会话自己的实例
  setServiceReadyHandler((sid: string) => {
    void drainAnnotate(sid);
  });
  // 启动监听并把地址交给 shell(spawn 时 --notify-url /notify/<sid>,就绪后 /api/notify-url 注册)
  // 1.9.0:通知按 URL 里的 sid 归属 → 彻底取代旧的「最后活跃会话」投递
  startNotifyServer((sid: string) => {
    void drainAnnotate(sid);
  })
    .then((p) => log.info(`[notify] listening 127.0.0.1:${p}/notify/<sid> (event-driven; per-session routing)`))
    .catch((e) => log.error("[notify] listen failed (only initial/service-ready drain available)", e as Error));
}

/** 拉取某会话待发送的记录并投递给**该会话**(推送触发;空结果不记日志) */
async function drainAnnotate(sid: string): Promise<void> {
  if (!sid) return;
  if (drainingSid.has(sid)) return;
  const c = annotateClient;
  if (!c) return;
  // 该会话实例未启动/已关闭:跳过(服务就绪或下次通知到达时会再次触发;**绝不因 drain 而 spawn**)
  if (!isRunning(sid)) return;
  drainingSid.add(sid);
  try {
    const r = await service.annotateConsumeSent({ _sessionId: sid });
    const records = r?.records || [];
    if (records.length === 0) return; // 空结果不刷屏
    const imgInfo = records
      .filter((x: any) => x.type === "screenshot" && x.image)
      .map((x: any) => `#${x.index}:${x.image.length}B`);
    log.info(`[drain] consume-sent -> ${records.length} record(s) ids=[${records.map((x: any) => x.index).join(",")}] images=[${imgInfo.join(",")}]`);
    const lines = ["User annotated via panel:", ""];
    for (const item of records) {
      const label =
        item.type === "screenshot"
          ? `[#${item.index}] 截图${item.url ? ` @ ${item.url}` : ""}`
          : `[#${item.index}] ${item.selector || "(no selector)"}${item.url ? ` @ ${item.url}` : ""}`;
      lines.push(`- ${label}${item.note ? `: ${item.note}` : ""}`);
      // 截图:落盘保留并写入文本路径。与参考实现(opencode-playwright-tool)一致 ——
      // 图片**不经 session.prompt**(服务端对 image/* part 返回 400),而是走
      // 「工具结果附件」(bt_screenshot)或「读记录返回缓存路径」(read_record_content)。
      if (item.type === "screenshot" && item.image) {
        const savedPath = saveShotImage(log, sid, item.index, item.image);
        if (savedPath) lines.push(`  (image saved: ${savedPath})`);
      }
      lines.push("");
    }
    const text = lines.join("\n");
    // 投递目标 = 通知 URL 里的 sid(1.9.0 会话隔离:取代旧的 hook-cache last-writer-wins)
    log.info(`[drain] sid=${sid} prompt parts=text-only textLen=${text.length}`);
    const res = await c.session.prompt({ path: { id: sid }, body: { noReply: false, parts: [{ type: "text", text }] } });
    const err = res?.error;
    if (err) {
      // 不再静默:写日志 + 页面通知(SDK 返回 {error} 不抛,必须显式检查)
      log.error(`[drain] prompt FAILED sid=${sid}: ${JSON.stringify(err).slice(0, 300)}`, new Error("prompt error"));
      try {
        await service.notify({ message: `发送记录失败: ${JSON.stringify(err).slice(0, 180)}`, type: "err" }, sid);
      } catch {}
    } else {
      log.info(`[drain] prompt OK sid=${sid} delivered ${records.length} record(s)`);
    }
  } catch (e) {
      log.error("annotate drain error", e as Error);
    } finally {
      drainingSid.delete(sid);
    }
}

// Export default for npm loader
export default opencodeBrowserTool;
