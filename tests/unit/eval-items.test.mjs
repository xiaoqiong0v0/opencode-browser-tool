// evaluate 多客户端项纯逻辑单测（node:test；先 `npm run build`，测试跑 dist 产物）
import test from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import {
  EVAL_ITEM_MAX_BYTES,
  EVAL_RESULT_MAX_CHARS,
  EVAL_TOTAL_MAX_BYTES,
  classifyEvalArgs,
  evalItemLabel,
  flattenEvalText,
  formatEvalReport,
  formatEvalValue,
  runEvalItems,
  truncateEvalText,
} from "../../dist/util/eval-items.js";

test("classify: @file: 与内联文本混合时按参数顺序保留", () => {
  const r = classifyEvalArgs(["@file:a.js", "console.log(1)", "@file:b.js"], false);
  assert.equal(r.ok, true);
  assert.deepEqual(
    r.items.map((i) => (i.kind === "file" ? `file:${i.path}` : `text:${i.text}`)),
    ["file:a.js", "text:console.log(1)", "file:b.js"],
  );
});

test("classify: --script 与位置参数同时给 → script_conflict", () => {
  assert.deepEqual(classifyEvalArgs(["1+1"], true), { ok: false, reason: "script_conflict" });
});

test("classify: 两者都没给 → no_items", () => {
  assert.deepEqual(classifyEvalArgs([], false), { ok: false, reason: "no_items" });
});

test("classify: 只有 --script（调用方走旧路径）不产生冲突", () => {
  assert.deepEqual(classifyEvalArgs([], true), { ok: false, reason: "no_items" });
});

test("evalItemLabel: file 带路径，text 不带", () => {
  assert.equal(evalItemLabel({ kind: "file", path: "scripts/hook.js" }), "file scripts/hook.js");
  assert.equal(evalItemLabel({ kind: "text", text: "1+1" }), "text");
});

test("formatEvalValue: 字符串原样，其它 JSON 序列化", () => {
  assert.equal(formatEvalValue("hi"), "hi");
  assert.equal(formatEvalValue(2), "2");
  assert.equal(formatEvalValue({ a: 1 }), '{"a":1}');
  assert.equal(formatEvalValue(undefined), "undefined");
  assert.equal(formatEvalValue(null), "null");
});

test("truncateEvalText / flattenEvalText: 截断到 500 且换行折叠成字面量", () => {
  assert.equal(EVAL_RESULT_MAX_CHARS, 500);
  const long = "x".repeat(600);
  const cut = truncateEvalText(long);
  assert.equal(cut.length, EVAL_RESULT_MAX_CHARS);
  assert.ok(cut.endsWith("…"));
  assert.equal(truncateEvalText("short"), "short");
  assert.equal(flattenEvalText("a\nb\r\nc"), "a\\nb\\nc");
});

test("字节上限常量: 单文件 256KiB / 合计 1MiB", () => {
  assert.equal(EVAL_ITEM_MAX_BYTES, 256 * 1024);
  assert.equal(EVAL_TOTAL_MAX_BYTES, 1024 * 1024);
});

test("formatEvalReport: 全部成功时首行不带后缀", () => {
  const text = formatEvalReport([{ label: "text", ok: true, ms: 0, output: "2" }], {
    total: 1,
    stopped: false,
    continued: false,
  });
  assert.equal(text, "evaluate: 1 item(s), 1 ok\n[1] text  ok  0ms  → 2");
});

test("formatEvalReport: 混合项按顺序渲染，失败标 FAILED 并带 (stop)", () => {
  const text = formatEvalReport(
    [
      { label: "file scripts/hook.js", ok: true, ms: 3, output: "1+1" },
      { label: "text", ok: true, ms: 0, output: "true" },
    ],
    { total: 3, stopped: true, continued: false },
  );
  const lines = text.split("\n");
  assert.equal(lines[0], "evaluate: 3 item(s), 2 ok (stop)");
  assert.match(lines[1], /^\[1\] file scripts\/hook\.js {2}ok {2}3ms {2}→ 1\+1$/);
  assert.match(lines[2], /^\[2\] text +ok {2}0ms {2}→ true$/);
});

test("formatEvalReport: 失败计数 + continue-on-error 标 (continued)", () => {
  const text = formatEvalReport(
    [
      { label: "text", ok: true, ms: 1, output: "1" },
      { label: "file scripts/bad.js", ok: false, ms: 2, output: "SyntaxError: bad" },
    ],
    { total: 2, stopped: false, continued: true },
  );
  const lines = text.split("\n");
  assert.equal(lines[0], "evaluate: 2 item(s), 1 ok, 1 failed (continued)");
  assert.match(lines[2], /^\[2\] file scripts\/bad\.js {2}FAILED {2}2ms {2}→ SyntaxError: bad$/);
});

test("formatEvalReport: 结果截断到 500 字符（含单行）", () => {
  const text = formatEvalReport([{ label: "text", ok: true, ms: 0, output: "y".repeat(600) }], {
    total: 1,
    stopped: false,
    continued: false,
  });
  const body = text.split("→ ")[1];
  assert.equal(body.length, EVAL_RESULT_MAX_CHARS);
  assert.ok(body.endsWith("…"));
});

test("formatEvalReport: 失败信息脱敏（绝对路径 → <path>）", () => {
  const text = formatEvalReport(
    [{ label: "file scripts/bad.js", ok: false, ms: 1, output: "SyntaxError: bad at C:\\Users\\me\\secret.js:1:2" }],
    { total: 1, stopped: true, continued: false },
  );
  assert.ok(!text.includes("C:\\Users"), "不得泄漏绝对路径");
  assert.ok(text.includes("<path>"));
});

test("formatEvalReport: 多行结果折叠成单行", () => {
  const text = formatEvalReport([{ label: "text", ok: true, ms: 0, output: "l1\nl2" }], {
    total: 1,
    stopped: false,
    continued: false,
  });
  assert.equal(text.split("\n").length, 2);
  assert.ok(text.endsWith("→ l1\\nl2"));
});

// ---- runEvalItems：mock 掉「service.evaluate」逐项断言（回归：调用方漏 await 时会拿到 Promise 本体）----

/** 构造一个记录调用顺序的假 evaluate，按脚本内容返回结果或抛错 */
function mockEval(table) {
  const calls = [];
  const evaluate = async (script) => {
    calls.push(script);
    const hit = table[script];
    if (hit instanceof Error) throw hit;
    return hit;
  };
  return { calls, evaluate };
}

/** 读文件的假实现：路径存在则返回内容，`!` 前缀视为越界错误 */
function mockRead(files) {
  return (path) => (path.startsWith("!") ? { error: `refusing to read ${path}` } : { content: files[path] ?? "" , bytes: 1 });
}

test("runEvalItems: 多文件 + 内联文本按顺序串行执行，逐项给出结果（须 await 后才是报告文本）", async () => {
  const { calls, evaluate } = mockEval({ "a.js": "from-a", "b.js": "from-b", "1+1": 2 });
  const pending = runEvalItems(
    [{ kind: "file", path: "a.js" }, { kind: "text", text: "1+1" }, { kind: "file", path: "b.js" }],
    { readFile: mockRead({ "a.js": "a.js", "b.js": "b.js" }), evaluate, continued: false },
  );
  assert.equal(typeof pending?.then, "function", "必须返回 Promise（调用方漏 await 会得到 [object Promise]）");
  const report = await pending;
  assert.equal(typeof report, "string");
  assert.ok(!report.includes("[object Promise]"));
  const lines = report.split("\n");
  assert.equal(lines[0], "evaluate: 3 item(s), 3 ok");
  assert.match(lines[1], /^\[1\] file a\.js {2}ok {2}\d+ms {2}→ from-a$/);
  assert.match(lines[2], /^\[2\] text +ok {2}\d+ms {2}→ 2$/);
  assert.match(lines[3], /^\[3\] file b\.js {2}ok {2}\d+ms {2}→ from-b$/);
  assert.deepEqual(calls, ["a.js", "1+1", "b.js"], "脚本按参数顺序逐项执行");
});

test("runEvalItems: 文件项读取失败 → 整次不执行任何脚本，原样返回错误文案", async () => {
  const { calls, evaluate } = mockEval({ x: 1 });
  const report = await runEvalItems(
    [{ kind: "text", text: "x" }, { kind: "file", path: "!../etc/passwd" }],
    { readFile: mockRead({}), evaluate, continued: false },
  );
  assert.equal(report, "refusing to read !../etc/passwd");
  assert.deepEqual(calls, [], "读取预检失败时不得执行任何脚本");
});

test("runEvalItems: 默认遇错即停（stop），报告仍显示计划总数", async () => {
  const { calls, evaluate } = mockEval({ one: 1, boom: new Error("SyntaxError: boom"), three: 3 });
  const report = await runEvalItems(
    [{ kind: "text", text: "one" }, { kind: "text", text: "boom" }, { kind: "text", text: "three" }],
    { readFile: mockRead({}), evaluate, continued: false },
  );
  const lines = report.split("\n");
  assert.equal(lines[0], "evaluate: 3 item(s), 1 ok, 1 failed (stop)");
  assert.match(lines[2], /FAILED {2}\d+ms {2}→ SyntaxError: boom$/);
  assert.deepEqual(calls, ["one", "boom"], "失败后不再执行后续项");
});

test("runEvalItems: --continue-on-error 时逐项执行到底并标 (continued)", async () => {
  const { calls, evaluate } = mockEval({ one: 1, boom: new Error("bad"), three: 3 });
  const report = await runEvalItems(
    [{ kind: "text", text: "one" }, { kind: "text", text: "boom" }, { kind: "text", text: "three" }],
    { readFile: mockRead({}), evaluate, continued: true },
  );
  const lines = report.split("\n");
  assert.equal(lines.length, 4);
  assert.equal(lines[0], "evaluate: 3 item(s), 2 ok, 1 failed (continued)");
  assert.match(lines[3], /→ 3$/);
  assert.deepEqual(calls, ["one", "boom", "three"]);
});

test("回归守卫：evaluate 命令必须 await 多客户端项执行（漏 await 会返回 [object Promise]）", () => {
  // 该 bug 只在调用点出现（runEvalItems 本身是 async），故直接读构建产物做静态守卫：
  // 断言 dist/index.js 里拼接 tn 的那一处带 await。
  const source = readFileSync(new URL("../../dist/index.js", import.meta.url), "utf-8");
  assert.match(source, /await\s+runEvalCommand\(a, parsed\.items\)/, "evaluate 多客户端项必须 await，否则输出 [object Promise]");
});


