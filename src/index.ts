import { existsSync, rmSync } from "fs";
import { join } from "path";
import { loadConfig, getConfig, getBrowsersDir } from "./config/index.js";
import { registerLocale, t } from "./i18n/index.js";
import en from "./i18n/en.js";
import zh from "./i18n/zh.js";
import { startService, service } from "./client.js";
import type { Plugin } from "@opencode-ai/plugin";
import createLogger from "@xiaoqiong0v0/opencode-plugin-logger";

const log = createLogger("browser-tool");
const _t = (k: string) => t(k);
const _tf = (key: string, vars?: Record<string, string>) => {
  let s = t(key);
  if (vars) for (const [vk, vv] of Object.entries(vars)) s = s.replace(`{${vk}}`, vv);
  return s;
};
const _exec = (fn: (a: any) => Promise<any>) => async (a: any, ctx?: any) => {
  return fn({ ...a, _sessionId: ctx?.sessionID || "" });
};

export const opencodeBrowserTool: Plugin = async ({ client, worktree }) => {
  registerLocale("en", en);
  registerLocale("zh", zh);
  loadConfig(worktree);
  const config = getConfig();

  try {
    log.loaded();
    await startService(config.nodePath || "", getBrowsersDir(), config.sessionIsolation, config.browserType);

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
          bt_navigate: {
            description: _t("tool.navigate.desc"),
            args: {
              url: { type: "string", description: _t("tool.navigate.arg.url") },
              headless: { type: "boolean", description: _t("tool.navigate.arg.headless") },
              browserType: {
                type: "string",
                description: "Browser engine: chromium, firefox, webkit (optional, keep current if omitted)",
              },
            },
            execute: _exec(async (a) => {
              const r = await service.navigate(a);
              if (r.installError) return `Installation failed: ${r.error}. Call bt_navigate again to retry.`;
              return r.installing
                ? _tf("msg.browser.installing", { browser: r.installing, progress: r.progress || "" })
                : _tf("msg.navigate.done", { url: r.url });
            }),
          },
          bt_click: {
            description: _t("tool.click.desc"),
            args: { selector: { type: "string", description: _t("tool.click.arg.selector") } },
            execute: _exec(async (a) => {
              await service.click(a);
              return "Clicked";
            }),
          },
          bt_fill: {
            description: _t("tool.fill.desc"),
            args: {
              selector: { type: "string", description: _t("tool.fill.arg.selector") },
              value: { type: "string", description: _t("tool.fill.arg.value") },
            },
            execute: _exec(async (a) => {
              await service.fill(a);
              return "Filled";
            }),
          },
          bt_clear: {
            description: _t("tool.clear.desc"),
            args: { selector: { type: "string", description: _t("tool.clear.arg.selector") } },
            execute: _exec(async (a) => {
              await service.clear(a);
              return "Cleared";
            }),
          },
          bt_select: {
            description: _t("tool.select.desc"),
            args: {
              selector: { type: "string", description: _t("tool.select.arg.selector") },
              value: { type: "string", description: _t("tool.select.arg.value") },
            },
            execute: _exec(async (a) => {
              await service.select(a);
              return "Selected";
            }),
          },
          bt_hover: {
            description: _t("tool.hover.desc"),
            args: { selector: { type: "string", description: _t("tool.hover.arg.selector") } },
            execute: _exec(async (a) => {
              await service.hover(a);
              return "Hovered";
            }),
          },
          bt_drag: {
            description: _t("tool.drag.desc"),
            args: {
              sourceSelector: { type: "string", description: _t("tool.drag.arg.sourceSelector") },
              targetSelector: { type: "string", description: _t("tool.drag.arg.targetSelector") },
            },
            execute: _exec(async (a) => {
              await service.drag(a);
              return "Dragged";
            }),
          },
          bt_press_key: {
            description: _t("tool.press_key.desc"),
            args: {
              key: { type: "string", description: _t("tool.press_key.arg.key") },
              selector: { type: "string", description: _t("tool.press_key.arg.selector") },
            },
            execute: _exec(async (a) => {
              await service.pressKey(a);
              return _tf("msg.press_key.done", { key: a.key });
            }),
          },
          bt_upload_file: {
            description: _t("tool.upload_file.desc"),
            args: {
              selector: { type: "string", description: _t("tool.upload_file.arg.selector") },
              filePath: { type: "string", description: _t("tool.upload_file.arg.filePath") },
            },
            execute: _exec(async (a) => {
              await service.uploadFile(a);
              return "Uploaded";
            }),
          },
          bt_screenshot: {
            description: _t("tool.screenshot.desc"),
            args: { selector: { type: "string", description: _t("tool.screenshot.arg.selector") } },
            execute: _exec(async (a) => {
              const r = await service.screenshot(a);
              return {
                output: `Screenshot taken${a.selector ? ` (element: ${a.selector})` : " (full page)"}`,
                attachments: [{ type: "file" as const, mime: "image/png", url: `data:image/png;base64,${r.__buffer}` }],
              };
            }),
          },
          bt_evaluate: {
            description: _t("tool.evaluate.desc"),
            args: { script: { type: "string", description: _t("tool.evaluate.arg.script") } },
            execute: _exec(async (a) => {
              const r = await service.evaluate(a);
              return JSON.stringify(r, null, 2);
            }),
          },
          bt_get_visible_text: {
            description: _t("tool.get_visible_text.desc"),
            args: { selector: { type: "string", description: _t("tool.get_visible_text.arg.selector") } },
            execute: _exec(async (a) => {
              const r = await service.visibleText(a);
              return r || "(no visible text)";
            }),
          },
          bt_get_visible_html: {
            description: _t("tool.get_visible_html.desc"),
            args: {
              selector: { type: "string", description: _t("tool.get_visible_html.arg.selector") },
              removeScripts: { type: "boolean", description: _t("tool.get_visible_html.arg.removeScripts") },
              removeComments: { type: "boolean", description: _t("tool.get_visible_html.arg.removeComments") },
              maxLength: { type: "number", description: _t("tool.get_visible_html.arg.maxLength") },
            },
            execute: _exec(async (a) => await service.visibleHtml(a)),
          },
          bt_console_logs: {
            description: _t("tool.console_logs.desc"),
            args: {
              type: { type: "string", description: _t("tool.console_logs.arg.type") },
              search: { type: "string", description: _t("tool.console_logs.arg.search") },
              limit: { type: "number", description: _t("tool.console_logs.arg.limit") },
              clear: { type: "boolean", description: _t("tool.console_logs.arg.clear") },
            },
            execute: _exec(async (a) => {
              const r = await service.consoleLogs(a);
              return (r as string[]).join("\n") || "(no logs)";
            }),
          },
          bt_go_back: {
            description: _t("tool.go_back.desc"),
            args: {},
            execute: _exec(async () => {
              const r = await service.goBack();
              return `Went back, current URL: ${r.url}`;
            }),
          },
          bt_go_forward: {
            description: _t("tool.go_forward.desc"),
            args: {},
            execute: _exec(async () => {
              const r = await service.goForward();
              return `Went forward, current URL: ${r.url}`;
            }),
          },
          bt_resize: {
            description: _t("tool.resize.desc"),
            args: {
              width: { type: "number", description: _t("tool.resize.arg.width") },
              height: { type: "number", description: _t("tool.resize.arg.height") },
            },
            execute: _exec(async (a) => {
              await service.resize(a);
              return "Resized";
            }),
          },
          bt_set_device: {
            description: _t("tool.set_device.desc"),
            args: { name: { type: "string", description: _t("tool.set_device.arg.name") } },
            execute: _exec(async (a) => {
              if (!a.name) {
                const list = await service.deviceList();
                const lines = ["Available device presets:", ""];
                for (const d of list.devices)
                  lines.push(`  ${d.name}  ${d.width}x${d.height}${d.ua ? `  ${d.ua.slice(0, 60)}...` : "  (default UA)"}`);
                return lines.join("\n");
              }
              const r = await service.device({ name: a.name });
              return _tf("msg.device.set", {
                name: r.device,
                size: `${r.width}x${r.height}`,
                ua: r.ua === "(default)" ? "(default)" : r.ua,
              });
            }),
          },
          bt_devtools: {
            description: _t("tool.devtools.desc"),
            args: { action: { type: "string", description: _t("tool.devtools.arg.action") } },
            execute: _exec(async (a) => {
              const r = await service.devtools({ action: a.action || "toggle" });
              return r.open ? _t("msg.devtools.open") : _t("msg.devtools.closed");
            }),
          },
          bt_reload: {
            description: _t("tool.reload.desc"),
            args: {},
            execute: _exec(async () => {
              const r = await service.reload();
              return `Page reloaded: ${r.url}`;
            }),
          },
          bt_close: {
            description: _t("tool.close.desc"),
            args: {},
            execute: _exec(async () => {
              await service.close();
              return "Browser closed";
            }),
          },
          bt_show_notification: {
            description: _t("tool.show_notification.desc"),
            args: {
              message: { type: "string", description: _t("tool.show_notification.arg.message") },
              type: { type: "string", description: _t("tool.show_notification.arg.type") },
            },
            execute: _exec(async (a) => {
              await service.notify(a);
              return "Notification shown";
            }),
          },
          bt_scroll: {
            description: _t("tool.scroll.desc"),
            args: {
              direction: { type: "string", description: _t("tool.scroll.arg.direction") },
              amount: { type: "number", description: _t("tool.scroll.arg.amount") },
            },
            execute: _exec(async (a) => {
              await service.scroll(a);
              return _tf("msg.scroll.done", { dir: a.direction || "down", amount: String(a.amount || 300) });
            }),
          },
          bt_wait_for_selector: {
            description: _t("tool.wait_for_selector.desc"),
            args: {
              selector: { type: "string", description: _t("tool.wait_for_selector.arg.selector") },
              timeout: { type: "number", description: _t("tool.wait_for_selector.arg.timeout") },
            },
            execute: _exec(async (a) => {
              await service.waitForSelector(a);
              return _t("msg.wait_selector.found");
            }),
          },
          bt_click_and_switch_tab: {
            description: _t("tool.click_and_switch_tab.desc"),
            args: { selector: { type: "string", description: _t("tool.click_and_switch_tab.arg.selector") } },
            execute: _exec(async (a) => {
              const r = await service.clickSwitchTab(a);
              return _tf(r.switched ? "msg.click_switch.done" : "msg.click_switch.clicked", {
                url: r.url || "",
                selector: a.selector,
              });
            }),
          },
          bt_iframe_click: {
            description: _t("tool.iframe_click.desc"),
            args: {
              iframeSelector: { type: "string", description: _t("tool.iframe_click.arg.iframeSelector") },
              selector: { type: "string", description: _t("tool.iframe_click.arg.selector") },
            },
            execute: _exec(async (a) => {
              await service.iframeClick(a);
              return "Clicked in iframe";
            }),
          },
          bt_iframe_fill: {
            description: _t("tool.iframe_fill.desc"),
            args: {
              iframeSelector: { type: "string", description: _t("tool.iframe_fill.arg.iframeSelector") },
              selector: { type: "string", description: _t("tool.iframe_fill.arg.selector") },
              value: { type: "string", description: _t("tool.iframe_fill.arg.value") },
            },
            execute: _exec(async (a) => {
              await service.iframeFill(a);
              return "Filled in iframe";
            }),
          },
          bt_save_as_pdf: {
            description: _t("tool.save_as_pdf.desc"),
            args: {},
            execute: _exec(async () => {
              await service.pdf();
              return "PDF saved";
            }),
          },
          bt_get_browser_status: {
            description: _t("tool.browser_status.desc"),
            args: {},
            execute: _exec(async () => {
              const s = await service.status();
              if (s.installing && Object.keys(s.installing).length > 0) {
                const info = Object.entries(s.installing)
                  .map(([b, p]) => `${b} (${p})`)
                  .join(", ");
                return _tf("msg.browser.installing", { browser: info, progress: "" });
              }
              if (!s.open) return _t("msg.browser.closed");
              return _tf("msg.browser.open", { title: s.title, url: s.url, tabs: String(s.tabs) });
            }),
          },
          bt_list_tabs: {
            description: _t("tool.list_tabs.desc"),
            args: {},
            execute: _exec(async () => {
              const pages = (await service.tabs()) as any[];
              return `Tabs (${pages.length}):\n${pages.map((p, i) => `[${i}] ${p.url}`).join("\n")}`;
            }),
          },
          bt_switch_tab: {
            description: _t("tool.switch_tab.desc"),
            args: { index: { type: "number", description: _t("tool.switch_tab.arg.index") } },
            execute: _exec(async (a) => {
              const r = await service.switchTab(a);
              return _tf("msg.tab.switched", { idx: String(a.index), url: r.url });
            }),
          },
          bt_new_tab: {
            description: _t("tool.new_tab.desc"),
            args: { url: { type: "string", description: _t("tool.new_tab.arg.url") } },
            execute: _exec(async (a) => {
              const r = await service.newTab(a);
              return _tf("msg.tab.new", { url: r.url });
            }),
          },
          bt_close_tab: {
            description: _t("tool.close_tab.desc"),
            args: { index: { type: "number", description: _t("tool.close_tab.arg.index") } },
            execute: _exec(async (a) => {
              await service.closeTab(a);
              return a.index !== undefined
                ? _tf("msg.tab.closed", { idx: String(a.index) })
                : _t("msg.tab.closed_current");
            }),
          },
          bt_get_element_state: {
            description: _t("tool.element_state.desc"),
            args: { selector: { type: "string", description: _t("tool.element_state.arg.selector") } },
            execute: _exec(async (a) => {
              const r: any = await service.elementState(a);
              if (!r || !r.exists) return _tf("msg.element.not_found", { selector: a.selector });
              return `Element <${r.tag}>: ${a.selector}\nVisible: ${r.visible}${r.text ? `\nText: ${r.text}` : ""}\nRect: ${r.rect.x},${r.rect.y} ${r.rect.w}x${r.rect.h}`;
            }),
          },
          bt_scroll_to_element: {
            description: _t("tool.scroll_to_element.desc"),
            args: { selector: { type: "string", description: _t("tool.scroll_to_element.arg.selector") } },
            execute: _exec(async (a) => {
              await service.scrollToElement(a);
              return "Scrolled to element";
            }),
          },
          bt_get_dropdown_options: {
            description: _t("tool.dropdown_options.desc"),
            args: { selector: { type: "string", description: _t("tool.dropdown_options.arg.selector") } },
            execute: _exec(async (a) => {
              const r = (await service.dropdownOptions(a)) as any[];
              if (!r) return _tf("msg.select.not_found", { selector: a.selector });
              return r.map((o: any) => `${o.selected ? "* " : "  "}${o.value}: ${o.text}`).join("\n");
            }),
          },
          bt_custom_user_agent: {
            description: _t("tool.user_agent.desc"),
            args: { userAgent: { type: "string", description: _t("tool.user_agent.arg.userAgent") } },
            execute: _exec(async (a) => {
              await service.userAgent(a);
              return "User-Agent set";
            }),
          },
          bt_expect_response: {
            description: _t("tool.expect_response.desc"),
            args: { url: { type: "string", description: _t("tool.expect_response.arg.url") } },
            execute: _exec(async (a) => {
              await service.expectResponse({ urlPattern: a.url });
              return `Now expecting response matching: ${a.url}. Use bt_assert_response with the same pattern to check.`;
            }),
          },
          bt_assert_response: {
            description: _t("tool.assert_response.desc"),
            args: { id: { type: "string", description: _t("tool.assert_response.arg.id") } },
            execute: _exec(async (a) => {
              const r = await service.assertResponse({ id: a.id });
              if (r.matched) return `Response matched: ${r.url} (${r.status})`;
              return r.error || "No response yet";
            }),
          },
          bt_get_accessibility_tree: {
            description: _t("tool.accessibility_tree.desc"),
            args: {
              selector: { type: "string", description: _t("tool.accessibility_tree.arg.selector") },
              maxDepth: { type: "number", description: _t("tool.accessibility_tree.arg.maxDepth") },
            },
            execute: _exec(async (a) => {
              const s = await service.accessibility(a);
              if (!s) return "(no accessibility info)";
              return formatNode(s, 0, a.maxDepth ?? 8);
            }),
          },
          bt_list_records: {
            description: "List all annotation records in the panel",
            args: {},
            async execute() {
              try {
                const records = await service.annotateRecords();
                if (!records || records.length === 0) return "(no records)";
                return records.map((r: any) => {
                  const rect = r.rect ? ` rect=(${r.rect[0]},${r.rect[1]},${r.rect[2]},${r.rect[3]})` : "";
                  return `[${r.index}] ${r.selector}${rect}${r.note ? ` note="${r.note}"` : ""}`;
                }).join("\n");
              } catch { return "(no records)"; }
            },
          },
          bt_read_record_content: {
            description: _t("tool.read_record.desc"),
            args: { id: { type: "number", description: _t("tool.read_record.arg.id") } },
          async execute(a: any) {
            // V5:批注记录在 shell 侧,读取记录信息(截图内容后续支持)
            try {
              const records = await service.annotateRecords();
              const rec = (records || []).find((r: any) => r.index === a.id);
              if (!rec) return `(record #${a.id} not found)`;
              return `#${rec.index} ${rec.selector}\nrect: ${JSON.stringify(rec.rect)}\nnote: ${rec.note || "(none)"}`;
            } catch { return "(no records)"; }
          },
        },
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

/** 轮询批注发送队列:面板"发送全部"/截图"发送"后,拉取记录推送当前对话 */
function startAnnotatePoller(client: any, log: any): void {
  const POLL_MS = 2000;
  const timer = setInterval(async () => {
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
            ? `[#${item.index}] 截图`
            : `[#${item.index}] ${item.selector || "(no selector)"}`;
        lines.push(`- ${label}${item.note ? `: ${item.note}` : ""}`);
        lines.push("");
      }
      const text = lines.join("\n");
      const bodyParts: any[] = [{ type: "text", text }];
      for (const p of parts) bodyParts.push(p);
      const sessions = await client.session.list();
      if (sessions?.data?.length) {
        await client.session.prompt({
          path: { id: sessions.data[0].id },
          body: { noReply: false, parts: bodyParts },
        });
      }
    } catch {
      // 服务未就绪/已退出,停止轮询
      clearInterval(timer);
    }
  }, POLL_MS);
  // 不阻塞进程退出
  if (typeof (timer as any).unref === "function") (timer as any).unref();
}

// Export default for npm loader
export default opencodeBrowserTool;
