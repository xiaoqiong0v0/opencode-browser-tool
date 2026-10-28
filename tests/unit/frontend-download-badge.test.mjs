// 工具栏下载角标纯逻辑单测（frontend/src/download-badge.ts）
// 行为契约：角标只显示「进行中」数量（0 → 隐藏）；新下载开始（progress）→ 脉冲 + title 换成文件名。
// 直接导入 TS 源（Node 24 默认类型擦除），不起浏览器、不依赖 Tauri。
import test from "node:test";
import assert from "node:assert/strict";

import {
  BADGE_MAX_COUNT,
  countInProgressDownloads,
  downloadEventTitle,
  downloadsBadgeText,
  isDownloadStart,
} from "../../frontend/src/download-badge.ts";

test("countInProgressDownloads: 统计各组 items 中的 progress", () => {
  const snapshot = {
    dir: "/d",
    groups: [
      { dir: "/d", items: [{ state: "progress" }, { state: "completed" }] },
      { dir: "/e", items: [{ state: "progress" }, { state: "progress" }, { state: "failed" }] },
    ],
  };
  assert.equal(countInProgressDownloads(snapshot), 3);
});

test("countInProgressDownloads: 无进行中 → 0", () => {
  assert.equal(countInProgressDownloads({ groups: [{ items: [{ state: "completed" }] }] }), 0);
});

test("countInProgressDownloads: 容忍缺失/非法结构（返回 0、不抛错）", () => {
  for (const bad of [undefined, null, {}, { groups: null }, { groups: "x" }, { groups: [{}] }, { groups: [{ items: 1 }] }]) {
    assert.equal(countInProgressDownloads(bad), 0);
  }
});

test("downloadsBadgeText: 0 / 负数 / 非数字 → 空串（隐藏角标）", () => {
  assert.equal(downloadsBadgeText(0), "");
  assert.equal(downloadsBadgeText(-1), "");
  assert.equal(downloadsBadgeText(Number.NaN), "");
  assert.equal(downloadsBadgeText(Number.POSITIVE_INFINITY), "");
});

test("downloadsBadgeText: 1..上限 原样、超过上限显示 99+", () => {
  assert.equal(downloadsBadgeText(1), "1");
  assert.equal(downloadsBadgeText(BADGE_MAX_COUNT), String(BADGE_MAX_COUNT));
  assert.equal(downloadsBadgeText(BADGE_MAX_COUNT + 1), `${BADGE_MAX_COUNT}+`);
  assert.equal(downloadsBadgeText(1000), `${BADGE_MAX_COUNT}+`);
});

test("isDownloadStart: 仅合法 payload 且 state=progress 时为 true", () => {
  assert.equal(isDownloadStart({ id: "dl-1", filename: "a.zip", state: "progress" }), true);
  for (const state of ["completed", "failed", "canceled"]) {
    assert.equal(isDownloadStart({ id: "dl-1", state }), false);
  }
  for (const bad of [undefined, null, {}, { id: "", state: "progress" }, { id: "dl-1", state: "weird" }]) {
    assert.equal(isDownloadStart(bad), false);
  }
});

test("downloadEventTitle: 取 filename 拼成「下载：<文件名>」", () => {
  assert.equal(downloadEventTitle({ id: "dl-1", filename: "报告.pdf", state: "progress" }), "下载：报告.pdf");
  assert.equal(downloadEventTitle({ id: "dl-1", filename: "  a.zip  ", state: "progress" }), "下载：a.zip");
});

test("downloadEventTitle: 缺 filename / 空白 / 非字符串 → null（保持原标题）", () => {
  for (const bad of [undefined, null, {}, { id: "dl-1" }, { id: "dl-1", filename: "   " }, { id: "dl-1", filename: 3 }]) {
    assert.equal(downloadEventTitle(bad), null);
  }
});
