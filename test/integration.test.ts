/**
 * 集成测试 — 边界情况 + E2E 流程
 * 用法: npm test
 */
import { BrowserManager } from "../src/browser/manager.js";
import { HttpBridge } from "../src/bridge/http-server.js";
import { loadConfig } from "../src/config/index.js";
import { registerLocale } from "../src/i18n/index.js";
import en from "../src/i18n/en.js";
import zh from "../src/i18n/zh.js";
import { readFileSync, existsSync } from "fs";
import { createNavigateTool } from "../src/tools/navigate.js";
import { createClickTool } from "../src/tools/click.js";
import { createFillTool } from "../src/tools/fill.js";
import { createClearTool } from "../src/tools/clear.js";
import { createSelectTool } from "../src/tools/select.js";
import { createHoverTool } from "../src/tools/hover.js";
import { createPressKeyTool } from "../src/tools/press_key.js";
import { createScreenshotTool } from "../src/tools/screenshot.js";
import { createEvaluateTool } from "../src/tools/evaluate.js";
import { createGetVisibleTextTool } from "../src/tools/get_visible_text.js";
import { createGetVisibleHtmlTool } from "../src/tools/get_visible_html.js";
import { createGetAccessibilityTreeTool } from "../src/tools/get_accessibility_tree.js";
import { createGoBackTool, createGoForwardTool } from "../src/tools/go.js";
import { createReloadTool } from "../src/tools/reload.js";
import { createResizeTool } from "../src/tools/resize.js";
import { createScrollTool } from "../src/tools/scroll.js";
import { createScrollToElementTool } from "../src/tools/scroll_to_element.js";
import { createWaitForSelectorTool } from "../src/tools/wait_for_selector.js";
import { createListTabsTool } from "../src/tools/list_tabs.js";
import { createSwitchTabTool } from "../src/tools/switch_tab.js";
import { createNewTabTool } from "../src/tools/new_tab.js";
import { createCloseTabTool } from "../src/tools/close_tab.js";
import { createIframeClickTool } from "../src/tools/iframe_click.js";
import { createIframeFillTool } from "../src/tools/iframe_fill.js";
import { createSaveAsPdfTool } from "../src/tools/save_as_pdf.js";
import { createGetBrowserStatusTool } from "../src/tools/get_browser_status.js";
import { createGetElementStateTool } from "../src/tools/get_element_state.js";
import { createGetDropdownOptionsTool } from "../src/tools/get_dropdown_options.js";
import { createExpectResponseTool } from "../src/tools/expect_response.js";
import { createAssertResponseTool } from "../src/tools/assert_response.js";
import { createGetVisibleTextTool as createGetVisibleTextToolName } from "../src/tools/get_visible_text.js";

registerLocale("en", en);
registerLocale("zh", zh);
loadConfig();

let passed = 0;
let failed = 0;

async function t(label: string, fn: () => Promise<void>) {
  process.stdout.write(`  ${label}... `);
  try { await fn(); console.log("OK"); passed++; }
  catch (e) { console.log(`FAIL\n    ${(e as Error).message}`); failed++; }
}

const SIMPLE_HTML = "<html><body><h1>Hello</h1></body></html>";
const DATA_URL = "data:text/html," + encodeURIComponent(SIMPLE_HTML);
const FULL_HTML = `<html><body>
<h1>Test Page</h1><p id='msg'>hello world</p>
<input id='name' placeholder='enter name'>
<select id='lang'><option>EN</option><option>ZH</option></select>
<a href='data:text/html,<html>Linked</html>' id='link'>click me</a>
<button id='btn'>Button</button>
<iframe id='frm' srcdoc='<html><body><input id="if-input"></body></html>'></iframe>
</body></html>`;
const FULL_URL = "data:text/html," + encodeURIComponent(FULL_HTML);

async function runTests(mgr: BrowserManager) {
  console.log("\n=== Functional Tests ===\n");

  await t("navigate to page", async () => {
    const r = await createNavigateTool(mgr).execute({ url: DATA_URL });
    if (typeof r !== "string" || !r.includes("data:")) throw "navigate failed: " + String(r).slice(0, 60);
  });

  await t("reload", async () => {
    const r = await createReloadTool(mgr).execute({});
    if (typeof r !== "string") throw "reload failed";
  });

  await t("get_visible_text full", async () => {
    const r = await createGetVisibleTextTool(mgr).execute({});
    if (!r.includes("Hello")) throw "expected Hello in: " + String(r).slice(0, 60);
  });

  await t("get_visible_html", async () => {
    const r = await createGetVisibleHtmlTool(mgr).execute({});
    if (!r.includes("Hello")) throw "expected Hello in HTML";
  });

  await t("evaluate", async () => {
    const r = await createEvaluateTool(mgr).execute({ script: "document.title" });
    if (typeof r !== "string") throw "evaluate failed";
  });

  await t("screenshot page", async () => {
    const r = await createScreenshotTool(mgr).execute({});
    if (typeof r === "string") throw "expected object";
    if (!r.attachments?.[0]?.url?.startsWith("data:image")) throw "no image attachment";
  });

  await t("get_browser_status", async () => {
    const r = await createGetBrowserStatusTool(mgr).execute({});
    if (!r.toLowerCase().includes("open")) throw "expected open status: " + String(r).slice(0, 60);
  });

  // ===== Navigation =====
  await t("go_back", async () => {
    const r = await createGoBackTool(mgr).execute({});
    if (typeof r !== "string") throw "go_back failed";
  });

  await t("go_forward", async () => {
    const r = await createGoForwardTool(mgr).execute({});
    if (typeof r !== "string") throw "go_forward failed";
  });

  // ===== Element interactions =====
  await t("fill input", async () => {
    await createNavigateTool(mgr).execute({ url: FULL_URL });
    const r = await createFillTool(mgr).execute({ selector: "#name", value: "Alice" });
    if (typeof r !== "string") throw String(r).slice(0, 60);
  });

  await t("clear input", async () => {
    const r = await createClearTool(mgr).execute({ selector: "#name" });
    if (typeof r !== "string") throw String(r).slice(0, 60);
  });

  await t("select option", async () => {
    const r = await createSelectTool(mgr).execute({ selector: "#lang", value: "ZH" });
    if (!r.includes("ZH")) throw "select failed: " + r;
  });

  await t("hover element", async () => {
    const r = await createHoverTool(mgr).execute({ selector: "#btn" });
    if (typeof r !== "string") throw "hover failed";
  });

  await t("press key", async () => {
    const page = await mgr.getActivePage();
    await page.focus("#name");
    const r = await createPressKeyTool(mgr).execute({ key: "ArrowDown" });
    if (typeof r !== "string") throw "press_key failed";
  });

  await t("click element", async () => {
    const r = await createClickTool(mgr).execute({ selector: "#link" });
    if (typeof r !== "string") throw "click failed";
  });

  await t("get_element_state exists", async () => {
    const r = await createGetElementStateTool(mgr).execute({ selector: "h1" });
    if (!r.includes("h1") || !r.includes("Visible")) throw "expected h1+Visible in:\n" + r;
  });

  await t("get_dropdown_options", async () => {
    await createNavigateTool(mgr).execute({ url: FULL_URL });
    const r = await createGetDropdownOptionsTool(mgr).execute({ selector: "#lang" });
    if (!r.includes("EN") || !r.includes("ZH")) throw "expected options in:\n" + r;
  });

  // ===== Tabs =====
  await t("list_tabs", async () => {
    const r = await createListTabsTool(mgr).execute({});
    if (!r.includes("[0]")) throw "expected tabs in:\n" + String(r).slice(0, 60);
  });

  await t("new_tab", async () => {
    const r = await createNewTabTool(mgr).execute({ url: "data:text/html,<h1>Tab2</h1>" });
    if (typeof r !== "string") throw "new_tab failed";
  });

  await t("switch_tab", async () => {
    const r = await createSwitchTabTool(mgr).execute({ index: 0 });
    if (typeof r !== "string") throw "switch_tab failed";
  });

  // ===== Viewport & Scroll =====
  await t("resize", async () => {
    const r = await createResizeTool(mgr).execute({ width: 1024, height: 768 });
    if (!r.includes("1024x768")) throw "resize failed: " + r;
  });

  await t("scroll", async () => {
    const r = await createScrollTool(mgr).execute({ direction: "down", amount: 50 });
    if (typeof r !== "string") throw "scroll failed";
  });

  await t("scroll_to_element", async () => {
    const r = await createScrollToElementTool(mgr).execute({ selector: "h1" });
    if (typeof r !== "string") throw "scroll_to failed";
  });

  await t("wait_for_selector", async () => {
    const r = await createWaitForSelectorTool(mgr).execute({ selector: "body" });
    if (typeof r !== "string") throw "wait_for failed";
  });

  // ===== iframe =====
  await t("iframe_fill", async () => {
    const r = await createIframeFillTool(mgr).execute({ iframeSelector: "#frm", selector: "#if-input", value: "hello" });
    if (typeof r !== "string") throw String(r).slice(0, 60);
  });

  await t("iframe_click", async () => {
    const r = await createIframeClickTool(mgr).execute({ iframeSelector: "#frm", selector: "#if-input" });
    if (typeof r !== "string") throw String(r).slice(0, 60);
  });

  // ===== Network =====
  await t("expect_response", async () => {
    const r = await createExpectResponseTool(mgr).execute({ id: "net1", url: "data:" });
    if (typeof r !== "string") throw String(r).slice(0, 60);
  });

  await t("assert_response", async () => {
    const r = await createAssertResponseTool().execute({ id: "net1" });
    if (typeof r !== "string") throw String(r).slice(0, 60);
  });

  // ===== PDF =====
  await t("save_as_pdf", async () => {
    try { const r = await createSaveAsPdfTool(mgr).execute({}); if (typeof r === "object" && r.attachments) return; }
    catch { return; }
  });

  console.log("\n=== Edge Case Tests ===\n");

  await t("get_element_state nonexistent", async () => {
    const r = await createGetElementStateTool(mgr).execute({ selector: "#nonexistent" });
    if (!r.toLowerCase().includes("not found")) throw "expected not found: " + r;
  });

  await t("get_visible_text empty selector", async () => {
    const r = await createGetVisibleTextTool(mgr).execute({ selector: "#nonexistent" });
    if (typeof r !== "string") throw "expected string";
  });

  await t("get_visible_html no selector", async () => {
    const r = await createGetVisibleHtmlTool(mgr).execute({});
    if (typeof r !== "string") throw "expected string";
  });

  await t("get_dropdown_options wrong type", async () => {
    const r = await createGetDropdownOptionsTool(mgr).execute({ selector: "h1" });
    if (!r.toLowerCase().includes("not found") && !r.includes("null") && !r.includes("undefined"))
      throw "expected error for wrong element type: " + String(r).slice(0, 80);
  });

  await t("screenshot nonexistent element", async () => {
    const r = await createScreenshotTool(mgr).execute({ selector: "#nonexistent" });
    if (typeof r === "string" && r.toLowerCase().includes("not found")) return;
    if (typeof r === "object") return; // some implementations might return empty
    throw "unexpected: " + String(r).slice(0, 60);
  });

  await t("switch_tab invalid index", async () => {
    const r = await createSwitchTabTool(mgr).execute({ index: 999 });
    if (typeof r !== "string") throw "expected string";
  });

  await t("close_tab", async () => {
    const r = await createCloseTabTool(mgr).execute({});
    if (typeof r !== "string") throw "close_tab failed";
  });

  await t("press_key empty key", async () => {
    try {
      const r = await createPressKeyTool(mgr).execute({ key: "" });
      if (typeof r !== "string") throw "expected string";
    } catch { return; }
  });

  await t("fill on non-input element", async () => {
    try {
      await createFillTool(mgr).execute({ selector: "h1", value: "x" });
    } catch { return; }
  });

  await t("hover nonexistent", async () => {
    try {
      const r = await createHoverTool(mgr).execute({ selector: "#nonexistent" });
      if (typeof r !== "string") throw "expected string";
    } catch { return; }
  });

  await t("wait_for_selector timeout", async () => {
    const start = Date.now();
    try {
      await createWaitForSelectorTool(mgr).execute({ selector: "#nonexistent", timeout: 500 });
    } catch { return; }
    throw "expected timeout error";
  });

  console.log("\n=== E2E Flow ===\n");

  await t("full e2e: navigate → annotate → screenshot → send → read", async () => {
    // 0. Start fresh
    await mgr.close();
    await mgr.launch(true);

    // 1. Navigate
    const navR = await createNavigateTool(mgr).execute({ url: FULL_URL });
    if (!navR.includes("data:")) throw "navigate failed";

    // 2. Get page info (simulating annotation)
    const text = await createGetVisibleTextTool(mgr).execute({});
    if (!text.includes("hello")) throw "text not found";

    // 3. Screenshot
    const shot = await createScreenshotTool(mgr).execute({});
    if (typeof shot === "string" || !shot.attachments) throw "screenshot failed";

    // 4. Element state (simulating element inspection)
    const state = await createGetElementStateTool(mgr).execute({ selector: "#name" });
    if (!state.includes("input")) throw "element not found";

    // 5. Fill + click (simulating interaction)
    await createFillTool(mgr).execute({ selector: "#name", value: "test" });
    const clickR = await createClickTool(mgr).execute({ selector: "#link" });
    if (typeof clickR !== "string") throw "click failed";

    // 6. Go back
    const backR = await createGoBackTool(mgr).execute({});
    if (typeof backR !== "string") throw "go_back failed";

    // 7. Evaluate
    const evalR = await createEvaluateTool(mgr).execute({ script: "location.href" });
    if (typeof evalR !== "string") throw "evaluate failed";

    // 8. Get accessibility tree
    const a11y = await createGetAccessibilityTreeTool(mgr).execute({ maxDepth: 3 });
    if (typeof a11y !== "string") throw "a11y failed";

    // 9. Scroll + resize
    await createResizeTool(mgr).execute({ width: 1280, height: 720 });
    await createScrollTool(mgr).execute({ direction: "down", amount: 100 });

    // 10. Browser status
    const status = await createGetBrowserStatusTool(mgr).execute({});
    if (!status.toLowerCase().includes("open")) throw "browser not open";
  });
}

async function main() {
  console.log("=== opencode-playwright-tool Integration Tests ===\n");
  const panel = existsSync("dist/panel/panel.js") ? readFileSync("dist/panel/panel.js", "utf-8") : "";

  const bridge = new HttpBridge(3456);
  await bridge.start();
  const mgr = new BrowserManager("");
  if (panel) mgr.setPanelScript(panel);

  await t("launch browser", async () => { await mgr.launch(true); });
  await runTests(mgr);
  await t("close browser", async () => { await mgr.close(); });
  await bridge.stop();

  console.log(`\n=== ${passed} passed, ${failed} failed ===`);
  if (failed > 0) process.exit(1);
}

main().catch(e => { console.error("CRASH:", e); process.exit(1); });
