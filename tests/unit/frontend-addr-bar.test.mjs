// 地址栏纯逻辑单测(frontend/src/addr-bar.ts)
// 行为契约:失焦时「输入为空 → 恢复当前标签 URL」;输入非空 / 拿不到 URL / 内部新标签页 → 不干预。
// 直接导入 TS 源(Node 24 默认类型擦除),不起浏览器、不依赖 Tauri。
import test from "node:test";
import assert from "node:assert/strict";

import { isNewTabUrl, resolveAddrBlurValue } from "../../frontend/src/addr-bar.ts";

test("resolveAddrBlurValue: 输入为空 + 有 URL → 返回该 URL(恢复)", () => {
  assert.equal(resolveAddrBlurValue("", "https://example.com/a?b=1"), "https://example.com/a?b=1");
});

test("resolveAddrBlurValue: 无 URL(缺失 / 空白)→ null(不动)", () => {
  for (const bad of [undefined, null, "", "   "]) {
    assert.equal(resolveAddrBlurValue("", bad), null);
  }
});

test("resolveAddrBlurValue: 输入非空 → null(保留用户输入)", () => {
  assert.equal(resolveAddrBlurValue("https://typed.example", "https://example.com"), null);
  assert.equal(resolveAddrBlurValue("  x  ", "https://example.com"), null);
});

test("resolveAddrBlurValue: 输入仅空白视为空 → 恢复", () => {
  assert.equal(resolveAddrBlurValue("   ", "https://example.com"), "https://example.com");
});

test("resolveAddrBlurValue: 当前标签是内部新标签页 → null(没有可恢复内容)", () => {
  for (const url of ["about:blank", "http://tauri.localhost/newtab.html", "tauri://localhost/index.html"]) {
    assert.equal(resolveAddrBlurValue("", url), null);
  }
});

test("isNewTabUrl: 识别 about:blank / newtab.html / tauri 内部来源", () => {
  assert.equal(isNewTabUrl("about:blank"), true);
  assert.equal(isNewTabUrl("http://tauri.localhost/newtab.html"), true);
  assert.equal(isNewTabUrl("tauri://localhost/newtab.html"), true);
  assert.equal(isNewTabUrl("https://example.com/newtab.html"), true);
  assert.equal(isNewTabUrl("https://example.com/index.html"), false);
  assert.equal(isNewTabUrl(""), false);
});
