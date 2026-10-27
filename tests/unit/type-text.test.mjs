// type 命令纯逻辑单测（node:test；先 `npm run build`，测试跑 dist 产物）
import test from "node:test";
import assert from "node:assert/strict";
import {
  TYPE_FILE_PREFIX,
  TYPE_TEXT_MAX_BYTES,
  formatTypeOutput,
  resolveTypeText,
  typeTextIssue,
} from "../../dist/util/type-text.js";

test("typeTextIssue: 空文本 → required", () => {
  assert.equal(typeTextIssue(""), "required");
});

test("typeTextIssue: 上限按字节计（8192 通过 / 8193 拒绝）", () => {
  assert.equal(TYPE_TEXT_MAX_BYTES, 8 * 1024);
  assert.equal(typeTextIssue("a".repeat(TYPE_TEXT_MAX_BYTES)), null);
  assert.equal(typeTextIssue("a".repeat(TYPE_TEXT_MAX_BYTES + 1)), "too_large");
});

test("typeTextIssue: 多字节字符按 UTF-8 字节判定（4096 个汉字 = 12 KiB）", () => {
  assert.equal(typeTextIssue("字".repeat(4096)), "too_large");
  assert.equal(typeTextIssue("字".repeat(2000)), null);
});

test("formatTypeOutput: 有 selector 时回显选择器", () => {
  assert.equal(formatTypeOutput(5, "#term", false), "typed 5 char(s) into #term");
});

test("formatTypeOutput: --enter 追加 + Enter", () => {
  assert.equal(formatTypeOutput(3, "#term", true), "typed 3 char(s) into #term + Enter");
});

test("formatTypeOutput: 无 selector 时写 focused element", () => {
  assert.equal(formatTypeOutput(1, "", false), "typed 1 char(s) into focused element");
  assert.equal(formatTypeOutput(0, "", true), "typed 0 char(s) into focused element + Enter");
});

test("resolveTypeText: 内联文本不触达文件读取", () => {
  let called = 0;
  const r = resolveTypeText("hello", () => {
    called += 1;
    return { content: "should not be read" };
  });
  assert.deepEqual(r, { text: "hello" });
  assert.equal(called, 0, "非 @file: 参数不得读取文件");
});

test("resolveTypeText: @file: 前缀剥离后交给读取器（与 evaluate 同一套 containment）", () => {
  const seen = [];
  const r = resolveTypeText(`${TYPE_FILE_PREFIX}scripts/hook.txt`, (path) => {
    seen.push(path);
    return { content: "from file" };
  });
  assert.deepEqual(seen, ["scripts/hook.txt"]);
  assert.deepEqual(r, { text: "from file" });
});

test("resolveTypeText: 读取器的越界拒绝原样透传（不吞错、不改写）", () => {
  const outside = "type: refusing to read @file outside the workspace / plugin temp directories: ../../etc/passwd";
  const r = resolveTypeText(`${TYPE_FILE_PREFIX}../../etc/passwd`, () => ({ error: outside }));
  assert.deepEqual(r, { error: outside });
});

test("resolveTypeText: 空路径 → required 文案", () => {
  const r = resolveTypeText(`${TYPE_FILE_PREFIX}   `, () => ({ content: "unused" }));
  assert.ok("error" in r);
  assert.match(r.error, /--text/);
});
