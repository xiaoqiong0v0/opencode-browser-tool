// `@file:` 通用层模式判定单测（node:test；先 `npm run build`，测试跑 dist 产物）
// 回归点：evaluate --script 曾因通用层只认插件 tmp，工作区内脚本被误拒（file_outside_tmp）
import test from "node:test";
import assert from "node:assert/strict";
import { fileResolutionMode } from "../../dist/util/file-resolver.js";

test("fileResolutionMode: evaluate --script 允许工作区", () => {
  assert.equal(fileResolutionMode("evaluate", "script"), "workspace");
});

test("fileResolutionMode: type --text 交给命令自身解析", () => {
  assert.equal(fileResolutionMode("type", "text"), "own");
});

test("fileResolutionMode: evaluate 其它参数仍走 tmp-only", () => {
  assert.equal(fileResolutionMode("evaluate", "scriptPath"), "tmp");
  assert.equal(fileResolutionMode("evaluate", "timeout"), "tmp");
});

test("fileResolutionMode: 其它命令一律 tmp-only（不得因本次改动放宽）", () => {
  for (const cmd of ["fill", "upload_file", "inject", "type", ""]) {
    assert.equal(fileResolutionMode(cmd, "value"), "tmp", `${cmd || "(empty)"} --value`);
  }
  assert.equal(fileResolutionMode("type", "text2"), "tmp");
});
