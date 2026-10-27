// 日志流输出格式化单测（node:test；先 `npm run build`，测试跑 dist 产物）
import test from "node:test";
import assert from "node:assert/strict";
import {
  formatConsoleItem,
  formatConsoleItems,
  formatNetworkItem,
  formatNetworkItems,
  displayUrl,
  shortInitiator,
  formatHeaders,
  droppedNote,
  renderLogText,
} from "../../dist/util/log-format.js";

test("console: 聚合计数 / phase / source", () => {
  const line = formatConsoleItem({
    level: "error",
    msg: "boom",
    count: 12,
    phase: "interactive",
    source: "cdp",
  });
  assert.equal(line, "[error] boom (x12) phase=interactive source=cdp");
});

test("console: 单条不显示 (x1)；缺省 level 回退 log", () => {
  assert.equal(
    formatConsoleItem({ msg: "hi", count: 1, phase: "load", source: "injected" }),
    "[log] hi phase=load source=injected",
  );
});

test("console: 异常条目只追加 stack 首行", () => {
  const line = formatConsoleItem({
    level: "exception",
    msg: "Uncaught TypeError: x",
    count: 1,
    phase: "action:click",
    source: "native",
    stack: "TypeError: x\n    at app.js:1:2\n    at app.js:3:4",
  });
  assert.match(line, /^\[exception\] Uncaught TypeError: x phase=action:click source=native\n {2}stack: TypeError: x$/);
  assert.ok(!line.includes("app.js:3"), "只展示 stack 首行");
});

test("network: status/method/url/计数/耗时/归因/phase（同源去 scheme，保留 host+path+query）", () => {
  const line = formatNetworkItem({
    method: "GET",
    url: "https://app.local/api/poll?since=1",
    status: 200,
    initiator: "script https://app.local/static/app.js:42",
    durationMs: 30,
    phase: "interactive",
    count: 487,
  });
  assert.equal(line, "[200] GET app.local/api/poll?since=1 (x487) 30ms last from static/app.js:42 phase=interactive");
});

test("network: 失败请求 status=0 + failure", () => {
  const line = formatNetworkItem({
    method: "POST",
    url: "https://app.local/api/submit",
    status: 0,
    initiator: "fetch https://app.local/assets/app.js:88",
    failure: "net::ERR_ABORTED",
    phase: "action:click",
    count: 3,
  });
  assert.equal(line, "[0]   POST app.local/api/submit (x3) failed: net::ERR_ABORTED from assets/app.js:88 phase=action:click");
});

test("network: status=0 且无 failure 时给通用失败文案", () => {
  assert.equal(
    formatNetworkItem({ method: "GET", url: "https://a/b", status: 0 }),
    "[0]   GET a/b request failed",
  );
});

test("network: Linux 无栈 initiator 原样保留，other/空值不输出 from", () => {
  assert.equal(shortInitiator("subresource"), "subresource");
  assert.equal(shortInitiator("document"), "document");
  assert.equal(shortInitiator("other"), "other");
  assert.equal(shortInitiator(""), "other");
  assert.equal(shortInitiator("script https://cdn/a/b.js:7"), "a/b.js:7");
  const line = formatNetworkItem({ method: "GET", url: "https://a/b", status: 200, initiator: "subresource" });
  assert.ok(line.includes("from subresource"));
  // `other` 无归因：不输出 from（也不输出 `from other`）
  assert.ok(!formatNetworkItem({ method: "GET", url: "https://a/b", status: 200, initiator: "other" }).includes("from"));
});

test("network: --include-headers 才渲染 headers（对象/字符串容错）", () => {
  const item = {
    method: "GET",
    url: "https://a/api",
    status: 200,
    headers: { "content-type": "application/json", "set-cookie": ["a=1"] },
  };
  assert.ok(!formatNetworkItem(item).includes("headers:"));
  assert.match(
    formatNetworkItem(item, { includeHeaders: true }),
    /headers: content-type: application\/json; set-cookie: a=1$/,
  );
  assert.equal(formatHeaders("x: y"), "x: y");
  assert.equal(formatHeaders(null), "");
});

test("displayUrl: 同源去 scheme（保留 host+path+query）", () => {
  const origin = "https://web.zhihjy.cn";
  assert.equal(displayUrl("https://web.zhihjy.cn/js/app.js", { origin }), "web.zhihjy.cn/js/app.js");
  assert.equal(displayUrl("https://web.zhihjy.cn/js/app.js?v=1", { origin }), "web.zhihjy.cn/js/app.js?v=1");
  // 未给 origin 时退化为去 scheme 形式（仍然带 host，绝不输出裸路径）
  assert.equal(displayUrl("https://a.com/x/y?q=1#h"), "a.com/x/y?q=1");
  assert.equal(displayUrl("https://a.com"), "a.com/");
});

test("displayUrl: 跨域保留完整 URL", () => {
  const origin = "https://web.zhihjy.cn";
  assert.equal(
    displayUrl("https://other.host/lib/x.js?a=1", { origin }),
    "https://other.host/lib/x.js?a=1",
  );
  // 端口/协议不同也算跨域
  assert.equal(displayUrl("http://web.zhihjy.cn/js/a.js", { origin }), "http://web.zhihjy.cn/js/a.js");
});

test("displayUrl: 超长保留 host + 末尾 60 字符", () => {
  const long = `https://web.zhihjy.cn/${"a".repeat(200)}?q=1`;
  const out = displayUrl(long);
  assert.ok(out.length < 120, out);
  assert.ok(out.startsWith("web.zhihjy.cn/…"), out);
  assert.ok(out.endsWith("?q=1"), out);
  assert.ok(!out.startsWith("/"), "不得输出裸路径");
});

test("displayUrl: 裸路径/不可解析入参不以 / 开头", () => {
  assert.equal(displayUrl("/js/app.js"), "./js/app.js");
  assert.equal(displayUrl("data:text/html,x"), "data:text/html,x");
  assert.equal(displayUrl(""), "");
});

test("displayUrl: 页面 origin 由同批 doc 条目推断（跨域保留完整）", () => {
  const items = [
    { method: "GET", url: "https://web.zhihjy.cn/index.html", status: 200, type: "doc" },
    { method: "GET", url: "https://web.zhihjy.cn/js/app.js", status: 200, type: "script" },
    { method: "OPTIONS", url: "https://other.api/v1/x", status: 200, type: "xhr" },
  ];
  const lines = formatNetworkItems(items);
  assert.ok(lines[1].includes("web.zhihjy.cn/js/app.js"), lines[1]);
  assert.ok(lines[2].includes("https://other.api/v1/x"), lines[2]);
});

test("shortInitiator: 只留最后两段 + :line", () => {
  assert.equal(
    shortInitiator("script https://web.zhihjy.cn/js/chunk-vendors.js:3498"),
    "js/chunk-vendors.js:3498",
  );
  assert.equal(shortInitiator("script https://web.zhihjy.cn/app.js:42"), "app.js:42");
  assert.equal(shortInitiator("xhr https://web.zhihjy.cn/api/v1/x:7"), "v1/x:7");
});

test("shortInitiator: 解析不出 / 裸路径 / :0 → other", () => {
  assert.equal(shortInitiator("BASE_DIGITAL:0"), "other");
  assert.equal(shortInitiator(":0"), "other");
  assert.equal(shortInitiator("script https://web.zhihjy.cn/js/app.js:0"), "other");
  assert.equal(shortInitiator(""), "other");
  assert.equal(shortInitiator("other"), "other");
  // 任何入参都不得以 `/` 开头（否则会被当绝对路径掩码）
  const samples = [
    "script /js/app.js:12",
    "script https://a/b/c.js:9",
    "/abs/path/x.js:3",
    "  ",
    "fetch https://a/",
  ];
  for (const s of samples) assert.ok(!shortInitiator(s).startsWith("/"), s);
});

test("network: 输出不含裸路径（回归 [200] GET <path>）", () => {
  const line = formatNetworkItem({
    method: "GET",
    url: "https://web.zhihjy.cn/js/app.js",
    status: 200,
    type: "script",
    initiator: "script https://web.zhihjy.cn/js/app.js:12",
    phase: "action:navigate",
  });
  assert.equal(line, "[200] GET web.zhihjy.cn/js/app.js from js/app.js:12 phase=action:navigate");
});

test("空 items 返回空数组（由命令层给空结果文案）", () => {
  assert.deepEqual(formatConsoleItems([]), []);
  assert.deepEqual(formatNetworkItems(undefined), []);
});

test("droppedNote: 只有 >0 才输出", () => {
  assert.equal(droppedNote(0), "");
  assert.equal(droppedNote(undefined), "");
  assert.equal(droppedNote(5), "(5 dropped by buffer limit)");
});

test("renderLogText: 多行 + 丢弃说明；空结果为空串", () => {
  assert.equal(renderLogText([], 0), "");
  assert.equal(renderLogText(["[log] hi"], 0), "[log] hi");
  assert.equal(renderLogText(["[log] hi"], 2), "[log] hi\n(2 dropped by buffer limit)");
});

test("renderLogText: 掩码绝对路径（Windows/POSIX/file URL），保留 http URL", () => {
  const text = renderLogText([
    "[error] open C:\\Users\\me\\secret.txt",
    "[error] read /home/me/secret.txt",
    "[error] at file:///C:/Users/me/app.js:1",
    "[log] fetch https://api.example.com/v1/poll",
  ]);
  assert.ok(!text.includes("C:\\Users\\me"), text);
  assert.ok(!text.includes("/home/me"), text);
  assert.ok(!text.includes("file:///C:"), text);
  assert.ok(text.includes("https://api.example.com/v1/poll"), "http URL 应保留");
});
