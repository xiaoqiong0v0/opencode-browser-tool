// 触摸命令参数校验（node:test；先 `npm run build`，测试跑 dist 产物）
import test from "node:test";
import assert from "node:assert/strict";
import {
  parsePointer,
  parsePointText,
  parseTouchActions,
  swipeIssue,
  tapIssue,
} from "../../dist/util/touch.js";

test("touch: pointer 缺省/合法/非法", () => {
  assert.deepEqual(parsePointer(undefined, "touch"), { ok: true, pointer: "touch" });
  assert.deepEqual(parsePointer("mouse", "touch"), { ok: true, pointer: "mouse" });
  assert.deepEqual(parsePointer("  TOUCH  ", "mouse"), { ok: true, pointer: "touch" });
  assert.deepEqual(parsePointer("Mouse", "touch"), { ok: true, pointer: "mouse" });
  assert.equal(parsePointer("pen", "touch").ok, false);
});

test("touch: 坐标文本解析", () => {
  assert.deepEqual(parsePointText(" 10, -20.5 "), { ok: true, x: 10, y: -20.5 });
  assert.equal(parsePointText("1").ok, false);
  assert.equal(parsePointText("a,2").ok, false);
  assert.equal(parsePointText("1,2,3").ok, false);
});

test("touch: actions 合法数组", () => {
  const r = parseTouchActions(
    '[{"down":[{"x":100,"y":200},{"selector":"#b"}]},{"wait":{"ms":600}},{"move":[{"x":100,"y":260},{"x":300,"y":260}],"steps":6},{"up":[]}]',
  );
  assert.equal(r.ok, true);
  assert.equal(r.actions.length, 4);
});

test("touch: actions 非法 JSON/空/结构错误", () => {
  assert.equal(parseTouchActions("not json").reason, "json");
  assert.equal(parseTouchActions("{}").reason, "not_array");
  assert.equal(parseTouchActions("[]").reason, "empty");
  assert.equal(parseTouchActions('[{"click":{}}]').reason, "shape");
  assert.equal(parseTouchActions('[{"down":[{"x":1,"y":2}]}]').reason, "missing_up");
  assert.equal(parseTouchActions('[{"down":[{"x":1,"y":2}]},{"wait":{"ms":1}}]').reason, "missing_up");
  assert.equal(parseTouchActions('[{"down":[{"x":1,"y":2}]},{"up":[]},{"move":[{"x":2,"y":2}]}]').reason, "shape");
  assert.equal(parseTouchActions('[{"down":[{"x":1,"y":2}]},{"up":[]},{"up":[]}]').reason, "shape");
  assert.equal(parseTouchActions('[{"down":[{"x":1,"y":2}]},{"move":[{"x":2,"y":2}],"steps":2},{"up":[]}]').ok, true);
  assert.equal(parseTouchActions('[{"down":[{"x":1,"y":2},{"x":3,"y":4},{"x":5,"y":6},{"x":7,"y":8},{"x":9,"y":10},{"x":11,"y":12}]},{"up":[]}]').reason, "shape");
  assert.equal(parseTouchActions('[{"down":[{"selector":"#a","align":"top"}]},{"up":[]}]').reason, "shape");
});

test("touch: tap 目标互斥/缺省", () => {
  assert.equal(tapIssue({ selector: "#a" }), null);
  assert.equal(tapIssue({ x: 10, y: 20 }), null);
  assert.equal(tapIssue({ selector: "#a", x: 1, y: 2 }), "selector_conflict");
  assert.equal(tapIssue({}), "target_required");
  assert.equal(tapIssue({ x: 1 }), "xy_pair");
  assert.equal(tapIssue({ x: 1, y: "abc" }), "xy_invalid");
  assert.equal(tapIssue({ x: "abc", y: 2 }), "xy_invalid");
  assert.equal(tapIssue({ selector: "#a", pointer: "pen" }), "pointer");
});

test("touch: swipe 参数校验", () => {
  assert.equal(swipeIssue({ from: "1,2", to: "3,4" }), null);
  assert.equal(swipeIssue({ from: "bad", to: "3,4" }), "from_invalid");
  assert.equal(swipeIssue({ from: "1,2", to: "bad" }), "to_invalid");
  assert.equal(swipeIssue({ from: "1,2", to: "3,4", steps: "0" }), "steps_invalid");
  assert.equal(swipeIssue({ from: "1,2", to: "3,4", duration: "-1" }), "duration_invalid");
  assert.equal(swipeIssue({ from: "1,2", to: "3,4", duration: "1.5" }), "duration_invalid");
});
