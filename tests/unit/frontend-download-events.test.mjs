// 面板下载事件决策的纯逻辑单测（frontend/src/download-events.ts）
// 行为契约：事件**完全不打扰用户** —— 不打开面板、不切视图、不展开、不滚动；
// 只有用户当前正处于「下载」视图时才返回 refresh（节流由调用方负责）。
// 直接导入 TS 源（Node 24 默认类型擦除），不起浏览器、不依赖 Tauri。
import test from "node:test";
import assert from "node:assert/strict";

import { decideDownloadsChanged } from "../../frontend/src/download-events.ts";

test("progress（新下载开始）在「下载」视图下 → 仅刷新", () => {
  const d = decideDownloadsChanged({ id: "dl-1", filename: "a.pdf", state: "progress" }, "downloads");
  assert.deepEqual(d, { id: "dl-1", state: "progress", action: "refresh" });
});

test("progress（新下载开始）在「记录」视图下 → 不刷新、不切视图", () => {
  const d = decideDownloadsChanged({ id: "dl-1", filename: "a.pdf", state: "progress" }, "records");
  assert.deepEqual(d, { id: "dl-1", state: "progress", action: "none" });
});

test("completed 在「下载」视图下 → 仅刷新", () => {
  const d = decideDownloadsChanged({ id: "dl-1", state: "completed" }, "downloads");
  assert.deepEqual(d, { id: "dl-1", state: "completed", action: "refresh" });
});

test("failed / canceled 在「下载」视图下 → 仅刷新", () => {
  for (const state of ["failed", "canceled"]) {
    const d = decideDownloadsChanged({ id: "dl-1", state }, "downloads");
    assert.deepEqual(d, { id: "dl-1", state, action: "refresh" });
  }
});

test("结束态在「记录」视图下 → 什么都不做", () => {
  for (const state of ["completed", "failed", "canceled"]) {
    const d = decideDownloadsChanged({ id: "dl-1", state }, "records");
    assert.deepEqual(d, { id: "dl-1", state, action: "none" });
  }
});

test("决策对象不含切视图/展开/滚动字段（回归：不打扰用户）", () => {
  const d = decideDownloadsChanged({ id: "dl-1", state: "progress" }, "records");
  assert.deepEqual(Object.keys(d ?? {}).sort(), ["action", "id", "state"]);
  assert.equal("forceView" in (d ?? {}), false);
  assert.equal("nextForcedId" in (d ?? {}), false);
});

test("同一 id 重复 progress 在「下载」视图下仍返回 refresh（合并交给调用方节流）", () => {
  const first = decideDownloadsChanged({ id: "dl-1", state: "progress" }, "downloads");
  const again = decideDownloadsChanged({ id: "dl-1", state: "progress" }, "downloads");
  assert.equal(first?.action, "refresh");
  assert.equal(again?.action, "refresh");
});

test("payload 额外字段不参与决策（只认 id / state）", () => {
  const d = decideDownloadsChanged(
    { id: "dl-2", state: "completed", url: "https://x/y", receivedBytes: 12, extra: true },
    "downloads",
  );
  assert.deepEqual(d, { id: "dl-2", state: "completed", action: "refresh" });
});

test("非法 payload 返回 null（缺失 id / 未知状态 / 非对象 / 类型不符）", () => {
  assert.equal(decideDownloadsChanged(undefined, "downloads"), null);
  assert.equal(decideDownloadsChanged(null, "downloads"), null);
  assert.equal(decideDownloadsChanged({}, "downloads"), null);
  assert.equal(decideDownloadsChanged({ id: "", state: "progress" }, "downloads"), null);
  assert.equal(decideDownloadsChanged({ id: 7, state: "progress" }, "downloads"), null);
  assert.equal(decideDownloadsChanged({ id: "dl-1", state: "weird" }, "downloads"), null);
});
