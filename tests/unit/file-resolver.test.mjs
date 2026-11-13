// `@file:` 两层解析分工的纯逻辑单测（node:test；先 `npm run build`，测试跑 dist 产物）
// 回归点：evaluate --script 曾因通用层只认插件 tmp，导致工作区内脚本被误拒（file_outside_tmp）
import test from "node:test";
import assert from "node:assert/strict";
import { usesOwnFileResolver } from "../../dist/util/file-resolver.js";

test("usesOwnFileResolver: evaluate --script 走命令自身解析", () => {
  assert.equal(usesOwnFileResolver("evaluate", "script"), true);
});

test("usesOwnFileResolver: type --text 走命令自身解析", () => {
  assert.equal(usesOwnFileResolver("type", "text"), true);
});

test("usesOwnFileResolver: evaluate 的其它参数仍走通用层", () => {
  assert.equal(usesOwnFileResolver("evaluate", "scriptPath"), false);
  assert.equal(usesOwnFileResolver("evaluate", "timeout"), false);
});

test("usesOwnFileResolver: 其它命令一律走通用层", () => {
  for (const cmd of ["fill", "upload_file", "inject", "type"]) {
    assert.equal(usesOwnFileResolver(cmd, "value"), false, `${cmd} --value`);
  }
  assert.equal(usesOwnFileResolver("type", "text2"), false);
  assert.equal(usesOwnFileResolver("", "script"), false);
});
