import { createServer } from "http";
import { resolve } from "path";
import { spawn } from "child_process";

const bpIdx = process.argv.indexOf("--browsers-path");
if (bpIdx >= 0) process.env.PLAYWRIGHT_BROWSERS_PATH = process.argv[bpIdx + 1];

const bIdx = process.argv.indexOf("--browser");
const browserName: string = bIdx >= 0 ? process.argv[bIdx + 1] : "chromium";
const pw = (await import("playwright")) as any;
const launcher = pw[browserName];

let browser: any = null;
let activePage: any = null;
let panelScript = "";
let browserClosedByUser = false;
let closingBrowser = false;
let sessionIsolation = process.argv.includes("--session-isolation");
const sessions = new Map<string, { context: any; page: any }>();
let consoleLogs: { type: string; text: string; timestamp: number }[] = [];
let pendingResponse: { pattern: string; matched: boolean; result: any }[] = [];
const installStates = new Map<string, "starting" | "downloading" | "installing" | "done" | "error">();
const installPromises = new Map<string, Promise<void>>();
const installErrors = new Map<string, string>();

function json(data: any, status = 200) {
  return { status, body: JSON.stringify(data) };
}

function error(msg: string, status = 400) {
  return json({ success: false, error: msg }, status);
}

function ok(data?: any) {
  return json({ success: true, data });
}

async function ensureBrowser(bt: string): Promise<void> {
  const pw = await import("playwright") as any;
  const l = pw[bt];
  if (!l) return;
  const exe = l.executablePath();
  const { existsSync } = await import("fs");
  if (existsSync(exe)) return;
  const { exec } = await import("child_process");
  const { dirname } = await import("path");
  const { fileURLToPath } = await import("url");
  const cli = resolve(dirname(fileURLToPath(import.meta.resolve("playwright"))), "cli.js");
  process.stderr.write(`[pw] Installing ${bt} (proxy: ${process.env.HTTPS_PROXY || "none"})...\n`);
  installStates.set(bt, "downloading");
  await new Promise<void>((resolve, reject) => {
    const child = spawn("node", [cli, "install", bt], { timeout: 300000 });
    let lastLine = "";
    child.stdout.on("data", (d: Buffer) => {
      const line = d.toString().trim();
      if (line) { lastLine = line; process.stderr.write(`[pw] ${line}\n`); installStates.set(bt, line.length > 10 ? "installing" : "downloading"); }
    });
    child.stderr.on("data", (d: Buffer) => {
      const line = d.toString().trim();
      if (line) { lastLine = line; process.stderr.write(`[pw] ${line}\n`); installStates.set(bt, "installing"); }
    });
    child.on("close", (code) => {
      if (code !== 0) { installStates.set(bt, "error"); reject(new Error(`Install failed (code ${code}): ${lastLine}`)); return; }
      installStates.set(bt, "done"); resolve();
    });
    child.on("error", (err) => { installStates.set(bt, "error"); reject(err); });
  });
}

function tryInstall(bt: string): any {
  const state = installStates.get(bt);
  if (state === "done") { installStates.delete(bt); installPromises.delete(bt); return null; }
  if (state === "error") {
    const err = installErrors.get(bt) || "Unknown error";
    installStates.delete(bt); installErrors.delete(bt); installPromises.delete(bt);
    return ok({ installError: bt, error: err });
  }
  if (state) return ok({ installing: bt, progress: state });
  installStates.set(bt, "starting"); installErrors.delete(bt);
  installPromises.set(bt, new Promise((r) => {
    setTimeout(async () => {
      installStates.set(bt, "installing");
      try { await ensureBrowser(bt); } catch (e: any) { installErrors.set(bt, e.message); installStates.set(bt, "error"); r(); return; }
      installStates.set(bt, "done"); r();
    }, 0);
  }));
  return ok({ installing: bt, progress: "starting" });
}

function matchResponse(respOrReq: any): void {
  try {
    const url = typeof respOrReq.url === "function" ? respOrReq.url() : respOrReq.url || "";
    const status = typeof respOrReq.status === "function" ? respOrReq.status() : 200;
    for (const p of pendingResponse) {
      if (!p.matched && url.includes(p.pattern)) {
        p.matched = true;
        p.result = { matched: true, url, status };
      }
    }
  } catch (e: any) { process.stderr.write("[pw] matchResponse error: " + e.message + "\n"); }
}

function setupPageListeners(pg: any) {
  pg.on("request", matchResponse);
  pg.on("response", matchResponse);
  pg.on("requestfinished", matchResponse);
}

function setupResponseListener(b: any) {
  b.on("targetcreated", async (target: any) => {
    try {
      const pg = await target.page();
      if (!pg) return;
      setupPageListeners(pg);
    } catch { }
  });
  for (const ctx of b.contexts()) {
    for (const pg of ctx.pages()) setupPageListeners(pg);
  }
}

async function getOrCreatePage(sessionId?: string, switchTo?: string, headless?: boolean) {
  if (browser && !browser.isConnected()) {
    browser = null;
    activePage = null;
    sessions.clear();
  }
  const bt = switchTo || browserName;
  if (!browser) {
    browserClosedByUser = false;
    const pw = await import("playwright") as any;
    const exe = pw[bt].executablePath();
    const { existsSync } = await import("fs");
    if (!existsSync(exe)) {
      const r = tryInstall(bt);
      if (r) return { __install: true, status: r.status, body: r.body };
    }
    process.stderr.write(`[pw] Launcher: ${pw[bt].executablePath()}\n`);
    process.stderr.write(`[pw] Data: ${process.env.PLAYWRIGHT_BROWSERS_PATH || "(default)"}\n`);
    await ensureBrowser(bt);
    try { browser = await pw[bt].launch({ headless: headless ?? false }); } catch (e: any) {
      process.stderr.write(`[pw] Launch failed: ${e.message}\n`);
      throw new Error(`Browser ${bt} auto-install failed. Try pw_navigate again, or manually copy the ${bt} folder from node_modules/playwright to ${process.env.PLAYWRIGHT_BROWSERS_PATH || "the browsers cache directory"}.`);
    }
    browser.on("disconnected", () => {
      browser = null;
      sessions.clear();
      activePage = null;
      if (!closingBrowser) browserClosedByUser = true;
      consoleLogs = [];
      pendingResponse = [];
    });
    setupResponseListener(browser);
  }

  if (activePage) {
    try {
      await activePage.evaluate("1");
    } catch {
      activePage = null;
    }
  }
  if (sessionIsolation && sessionId) {
    let entry = sessions.get(sessionId);
    if (!entry) {
      const ctx = await browser.newContext();
      entry = { context: ctx, page: await ctx.newPage() };
      sessions.set(sessionId, entry);
    }
    return entry.page;
  }
  if (!activePage) {
    const ctx = browser.contexts()[0] || (await browser.newContext());
    if (ctx.pages().length > 0) activePage = ctx.pages()[0];
    else activePage = await ctx.newPage();
    if (panelScript) ctx.addInitScript(panelScript).catch(() => { });
  }
  setupPageListeners(activePage);
  activePage.on("close", () => {
    if (browser) {
      const remaining = browser.contexts().flatMap((c: any) => c.pages());
      if (remaining.length === 0) browserClosedByUser = true;
    }
  });
  return activePage;
}

async function handleNavigate(sid: string, body: any) {
  if (body.browserType && body.browserType !== browserName && browser) {
    try { await browser.close(); } catch { }
    browser = null;
    activePage = null;
    sessions.clear();
    consoleLogs = [];
    pendingResponse = [];
  }
  if (browserClosedByUser) { browser = null; activePage = null; }
  browserClosedByUser = false;
  const switchTo = body.browserType && body.browserType !== browserName ? body.browserType : undefined;
  const p = await getOrCreatePage(sid, switchTo, body.headless);
  if (p && p.__install) return p;
  await p.goto(body.url, { waitUntil: "domcontentloaded" });
  return ok({ url: p.url() });
}

async function getAllPages() {
  if (!browser) return [];
  const all: any[] = [];
  for (const ctx of browser.contexts()) {
    all.push(...ctx.pages());
  }
  return all;
}

async function handler(url: string, body: any) {
  const sid = body._sessionId;
  try {
    if (url === "/api/status") {
      const installs: Record<string, string> = {};
      for (const [b, s] of installStates)
        if (s !== "done") installs[b] = s + (s === "error" && installErrors.has(b) ? ": " + installErrors.get(b) : "");
      const page = await getOrCreatePage(sid).catch(() => null);
      if (!page) return ok({ open: false, installing: installs });
      try {
        return ok({
          open: true,
          url: await page.url(),
          title: await page.title(),
          tabs: (await getAllPages()).length,
          installing: installs,
        });
      } catch {
        return ok({ open: false, installing: installs });
      }
    }

    if (url === "/api/panel-config") {
      panelScript = body.script || "";
      if (browser) for (const ctx of browser.contexts()) for (const p of ctx.pages()) p.addInitScript(panelScript).catch(() => { });
      return ok({ ok: true });
    }

    if (url === "/api/tabs") {
      const pages = await getAllPages();
      const list = pages.map((p: any) => {
        try {
          return { url: p.url(), title: p.title(), closed: p.isClosed() };
        } catch {
          return { url: "", title: "", closed: true };
        }
      });
      return ok(list);
    }

    if (url === "/api/close-session") {
      if (body._sessionId) {
        const entry = sessions.get(body._sessionId);
        if (entry) { try { await entry.context.close(); } catch { } sessions.delete(body._sessionId); }
      }
      return ok({ closed: true });
    }

    if (url.startsWith("/api/records")) {
      return json({ success: true }, 200);
    }

    if (browserClosedByUser && url !== "/api/navigate")
      throw new Error("Browser was closed manually. Use pw_navigate to reopen.");
    const page = ["/api/navigate", "/api/close"].includes(url) ? null : await getOrCreatePage(sid);

    if (url === "/api/navigate") {
      const result = await handleNavigate(sid, body);
      if (result) return result;
    }

    if (url === "/api/click") {
      await page.click(body.selector);
      return ok({ clicked: true });
    }

    if (url === "/api/fill") {
      await page.fill(body.selector, body.value);
      return ok({ filled: true });
    }

    if (url === "/api/clear") {
      await page.fill(body.selector, "");
      return ok({ cleared: true });
    }

    if (url === "/api/select") {
      await page.selectOption(body.selector, body.value);
      return ok({ selected: true });
    }

    if (url === "/api/hover") {
      await page.hover(body.selector);
      return ok({ hovered: true });
    }

    if (url === "/api/press-key") {
      if (body.selector) await page.focus(body.selector);
      await page.keyboard.press(body.key);
      return ok({ pressed: true });
    }

    if (url === "/api/drag") {
      await page.dragAndDrop(body.sourceSelector, body.targetSelector);
      return ok({ dragged: true });
    }

    if (url === "/api/upload-file") {
      await page.setInputFiles(body.selector, body.filePath);
      return ok({ uploaded: true });
    }

    if (url === "/api/screenshot") {
      let buf: Buffer;
      if (body.selector) {
        const el = await page.$(body.selector);
        if (!el) return error(`Element not found: ${body.selector}`);
        buf = await el.screenshot({ type: "png" });
      } else if (body.clip) {
        buf = await page.screenshot({ fullPage: false, type: "png", clip: body.clip });
      } else {
        buf = await page.screenshot({ fullPage: true, type: "png" });
      }
      return ok({ __buffer: buf.toString("base64") });
    }

    if (url === "/api/evaluate") {
      const result = await page.evaluate(body.script);
      return ok(result);
    }

    if (url === "/api/visible-text") {
      const text = await page.evaluate((sel: string) => {
        const root = sel ? document.querySelector(sel) : document.body;
        return root ? (root as HTMLElement).innerText || "" : "";
      }, body.selector || null);
      return ok(text || null);
    }

    if (url === "/api/visible-html") {
      const html = await page.evaluate(
        (opts: any) => {
          const root = opts.sel ? document.querySelector(opts.sel) : document.body;
          if (!root) return "Element not found";
          const clone = root.cloneNode(true) as HTMLElement;
          if (opts.removeScripts !== false)
            clone.querySelectorAll("script,style,link[rel='stylesheet']").forEach((e: any) => e.remove());
          let r = clone.outerHTML || clone.innerHTML || "";
          if (opts.removeComments) r = r.replace(/<!--[\s\S]*?-->/g, "");
          return r;
        },
        { sel: body.selector || null, removeScripts: body.removeScripts, removeComments: body.removeComments },
      );
      const maxLen = body.maxLength ?? 20000;
      return ok(html.length > maxLen ? html.slice(0, maxLen) + `\n\n(truncated, total length ${html.length})` : html);
    }

    if (url === "/api/element-state") {
      const result = await page.evaluate((sel: string) => {
        const el = document.querySelector(sel);
        if (!el) return { exists: false };
        const r = el.getBoundingClientRect();
        const st = getComputedStyle(el);
        return {
          exists: true,
          visible: r.width > 0 && r.height > 0 && st.visibility !== "hidden" && st.display !== "none",
          tag: el.tagName.toLowerCase(),
          text: (el.textContent || "").trim().slice(0, 100),
          rect: { x: Math.round(r.x), y: Math.round(r.y), w: Math.round(r.width), h: Math.round(r.height) },
          disabled: (el as any).disabled || false,
        };
      }, body.selector);
      return ok(result);
    }

    if (url === "/api/dropdown-options") {
      const opts = await page.evaluate((sel: string) => {
        const el = document.querySelector(sel);
        if (!el || el.tagName !== "SELECT") return null;
        return Array.from((el as HTMLSelectElement).options).map((o) => ({
          value: o.value,
          text: o.text,
          selected: o.selected,
        }));
      }, body.selector);
      return ok(opts);
    }

    if (url === "/api/wait-for-selector") {
      await page.waitForSelector(body.selector, { timeout: body.timeout ?? 10000 });
      return ok({ found: true });
    }

    if (url === "/api/scroll") {
      await page.evaluate((d: any) => window.scrollBy(d.dx, d.dy), { dx: 0, dy: body.amount ?? 300 });
      return ok({ scrolled: true });
    }

    if (url === "/api/scroll-to-element") {
      await page.evaluate((sel: string) => {
        const el = document.querySelector(sel);
        if (el) el.scrollIntoView({ behavior: "smooth", block: "center" });
      }, body.selector);
      return ok({ scrolled: true });
    }

    if (url === "/api/reload") {
      await page.reload();
      return ok({ url: page.url() });
    }

    if (url === "/api/go-back") {
      await page.goBack();
      return ok({ url: page.url() });
    }

    if (url === "/api/go-forward") {
      await page.goForward();
      return ok({ url: page.url() });
    }

    if (url === "/api/resize") {
      await page.setViewportSize({ width: body.width, height: body.height });
      return ok({ resized: true });
    }

    if (url === "/api/close") {
      if (browser) {
        try {
          await browser.close();
        } catch { }
        browser = null;
        activePage = null;
      }
      return ok({ closed: true });
    }

    if (url === "/api/tabs/new") {
      const ctx = browser.contexts()[0] || (await browser.newContext());
      const newPage = await ctx.newPage();
      if (body.url) await newPage.goto(body.url, { waitUntil: "domcontentloaded" });
      activePage = newPage;
      return ok({ url: newPage.url() });
    }

    if (url === "/api/tabs/switch") {
      const all = await getAllPages();
      const target = all[body.index];
      if (!target) return error(`Tab #${body.index} not found`);
      await target.bringToFront();
      activePage = target;
      return ok({ url: target.url() });
    }

    if (url === "/api/tabs/close") {
      const all = await getAllPages();
      const target = body.index !== undefined ? all[body.index] : page;
      if (!target) return error("Tab not found");
      await target.close();
      const remaining = await getAllPages();
      activePage = remaining[0] || null;
      return ok({ closed: true });
    }

    if (url === "/api/click-switch-tab") {
      const [newPage] = await Promise.all([
        page.waitForEvent("popup", { timeout: 10000 }).catch(() => null),
        page.click(body.selector),
      ]);
      if (newPage) {
        await newPage.waitForLoadState("domcontentloaded");
        activePage = newPage;
        return ok({ switched: true, url: newPage.url() });
      }
      return ok({ switched: false });
    }

    if (url === "/api/iframe-click") {
      const frame = page.frameLocator(body.iframeSelector);
      await frame.locator(body.selector).click();
      return ok({ clicked: true });
    }

    if (url === "/api/iframe-fill") {
      const frame = page.frameLocator(body.iframeSelector);
      await frame.locator(body.selector).fill(body.value);
      return ok({ filled: true });
    }

    if (url === "/api/user-agent") {
      const ctx = browser.contexts()[0] || (await browser.newContext());
      await ctx.setExtraHTTPHeaders({ "User-Agent": body.userAgent || body.device });
      await page.evaluate((ua: string) => {
        Object.defineProperty(navigator, "userAgent", { get: () => ua, configurable: true });
      }, body.userAgent || body.device);
      return ok({ set: true });
    }

    if (url === "/api/press-key") {
      if (body.selector) await page.focus(body.selector);
      await page.keyboard.press(body.key);
      return ok({ pressed: true });
    }

    if (url === "/api/accessibility") {
      const root = body.selector ? await page.$(body.selector) : undefined;
      const snapshot = await (page as any).accessibility.snapshot({ root });
      return ok(snapshot);
    }

    if (url === "/api/notify") {
      return ok({ notified: true });
    }

    if (url === "/api/pdf") {
      const buf = await page.pdf();
      return ok({ __buffer: buf.toString("base64") });
    }

    if (url === "/api/console-logs") {
      const type = body.type || "all";
      const search = body.search || "";
      let filtered = consoleLogs;
      if (type !== "all") filtered = filtered.filter((l) => l.type === type);
      if (search) filtered = filtered.filter((l) => l.text.includes(search));
      const result = filtered.slice(0, body.limit || 50).map((l) => l.text);
      if (body.clear) consoleLogs = [];
      return ok(result);
    }

    if (url === "/api/expect-response") {
      const pattern = body.urlPattern;
      pendingResponse.push({ pattern, matched: false, result: null });
      return ok({ id: pattern, pattern });
    }

    if (url === "/api/assert-response") {
      const entry = pendingResponse.find((p) => p.pattern === body.id);
      if (!entry) return ok({ matched: false, error: "no pending expectation" });
      return ok(entry.result || { matched: false, error: "pending" });
    }

    return error(`Not found: POST ${url}`, 404);
  } catch (e: any) {
    return error(e.message, 500);
  }
}

const server = createServer(async (req, res) => {
  res.setHeader("Access-Control-Allow-Origin", "*");
  res.setHeader("Access-Control-Allow-Methods", "GET, POST, OPTIONS");
  res.setHeader("Access-Control-Allow-Headers", "Content-Type");
  if (req.method === "OPTIONS") {
    res.writeHead(204);
    res.end();
    return;
  }

  let body = "";
  req.on("data", (c: Buffer) => (body += c.toString()));
  req.on("end", async () => {
    let parsed: any = {};
    if (body) {
      try {
        parsed = JSON.parse(body);
      } catch {
        parsed = {};
      }
    }
    try {
      const result = (await handler(req.url || "/", parsed)) as any;
      res.writeHead(result.status, { "Content-Type": "application/json" });
      res.end(result.body);
    } catch (e: any) {
      res.writeHead(500, { "Content-Type": "application/json" });
      res.end(JSON.stringify({ success: false, error: e.message }));
    }
  });
});

server.listen(0, "127.0.0.1", () => {
  const addr = server.address();
  const port = addr && typeof addr === "object" ? addr.port : 3456;
  process.stdout.write(String(port) + "\n");
});

// Graceful shutdown
process.on("SIGTERM", async () => {
  if (browser)
    try {
      await browser.close();
    } catch { }
  process.exit(0);
});
