// drag 参数互斥校验（node:test；先 `npm run build`，测试跑 dist 产物）
import test from "node:test";
import assert from "node:assert/strict";
import { dragArgIssue } from "../../dist/util/drag.js";

test("drag: 选择器 + 坐标组合是合法的", () => {
  assert.equal(dragArgIssue({ sourceSelector: "#a", targetPoint: "10,20" }), null);
  assert.equal(dragArgIssue({ sourcePoint: "1,2", targetSelector: "#b" }), null);
});

test("drag: 同一侧不能同时给选择器与坐标", () => {
  assert.equal(
    dragArgIssue({ sourceSelector: "#a", sourcePoint: "1,2", targetSelector: "#b" }),
    "source_conflict",
  );
  assert.equal(
    dragArgIssue({ sourceSelector: "#a", targetSelector: "#b", targetPoint: "1,2" }),
    "target_conflict",
  );
});

test("drag: 缺少 source/target 侧时给出明确原因", () => {
  assert.equal(dragArgIssue({}), "source_required");
  assert.equal(dragArgIssue({ targetSelector: "#b" }), "source_required");
  assert.equal(dragArgIssue({ sourceSelector: "#a" }), "target_required");
});

test("drag: --actions 与四类 source/target 参数互斥", () => {
  assert.equal(dragArgIssue({ actions: "[]" }), null);
  assert.equal(dragArgIssue({ actions: "[]", sourceSelector: "#a" }), "actions_conflict");
  assert.equal(dragArgIssue({ actions: [], targetPoint: "1,2" }), "actions_conflict");
  // null 也算「出现 actions」;类型错误由服务端报错
  assert.equal(dragArgIssue({ actions: null }), null);
  assert.equal(dragArgIssue({ actions: null, targetSelector: "#b" }), "actions_conflict");
});

test("drag: 空白字符串视为未提供", () => {
  assert.equal(
    dragArgIssue({ sourceSelector: "  ", sourcePoint: "1,2", targetSelector: "#b" }),
    null,
  );
});
