// 下载列表 / 下载目录（1.13.0）纯逻辑单测：模板展开、参数透传、分组输出、错误透传
// 只跑 dist 里的纯函数与 i18n 字典，不联网、不启动 shell。
import test from "node:test";
import assert from "node:assert/strict";
import {
  DEFAULT_DOWNLOAD_DIR_TEMPLATE,
  DEFAULT_DOWNLOAD_HISTORY,
  SNAME_MAX_LEN,
  expandDownloadDir,
  sanitizeSessionName,
  formatBytes,
  dirTail,
  formatDownloadRow,
  formatDownloads,
  buildDownloadsPayload,
  buildDownloadDirPayload,
  downloadsLimitIssue,
} from "../../dist/util/downloads.js";
import { registerLocale, tf } from "../../dist/i18n/index.js";
import en from "../../dist/i18n/en.js";
import zh from "../../dist/i18n/zh.js";

registerLocale("en", en);
registerLocale("zh", zh);

/** 测试用文案（与 en 同义，避免依赖语言包注册顺序） */
const LABELS = {
  empty: "(no downloads)",
  unknownSize: "unknown",
  states: { progress: "downloading", completed: "completed", failed: "failed", canceled: "canceled" },
};

/** 造一条下载记录（默认进行中） */
function item(over = {}) {
  return {
    id: "d1",
    url: "https://files.example.com/a.zip",
    filename: "a.zip",
    path: "",
    totalBytes: 0,
    receivedBytes: 0,
    state: "progress",
    startedAt: 1,
    ...over,
  };
}

test("默认模板与历史上限：模板含四个占位符，默认 64 条", () => {
  assert.equal(DEFAULT_DOWNLOAD_DIR_TEMPLATE, "<cache>/sessions/<profile>/<sid>/downloads");
  assert.equal(DEFAULT_DOWNLOAD_HISTORY, 64);
});

test("模板展开：<cache>/<profile>/<sid> 原样替换", () => {
  const dir = expandDownloadDir(DEFAULT_DOWNLOAD_DIR_TEMPLATE, {
    cacheDir: "C:\\cache",
    profile: "work",
    sid: "ses_1",
    sessionName: "ignored",
  });
  assert.equal(dir, "C:\\cache/sessions/work/ses_1/downloads");
});

test("<sname> 安全化：去掉 \\ / : * ? \" < > | 与控制字符并折叠空白", () => {
  assert.equal(sanitizeSessionName('a\\b/c:d*e?f"g<h>i|j\u0001k'), "abcdefghijk");
  assert.equal(sanitizeSessionName("Fix: login / bug\tnow"), "Fix login bug now");
  assert.equal(sanitizeSessionName("  trailing dots...  "), "trailing dots");
  assert.equal(sanitizeSessionName(""), "");
});

test("<sname> 截断到 64 字符", () => {
  const long = "x".repeat(200);
  assert.equal(SNAME_MAX_LEN, 64);
  assert.equal(sanitizeSessionName(long).length, 64);
});

test("<sname> 回落：标题为空用 sid，两者都空用 default", () => {
  const vars = { cacheDir: "/cache", profile: "default", sid: "ses_9" };
  assert.equal(expandDownloadDir("<cache>/<sname>", { ...vars, sessionName: "  " }), "/cache/ses_9");
  assert.equal(expandDownloadDir("<cache>/<sname>", { ...vars, sid: "", sessionName: "" }), "/cache/default");
});

test("字节格式化：紧凑单位，整数不带 .0", () => {
  assert.equal(formatBytes(0), "0B");
  assert.equal(formatBytes(-1), "0B");
  assert.equal(formatBytes(700 * 1024), "700KB");
  assert.equal(formatBytes(3 * 1024 * 1024), "3MB");
  assert.equal(formatBytes(12 * 1024 * 1024), "12MB");
  assert.equal(formatBytes(1.25 * 1024 * 1024), "1.3MB");
});

test("请求体透传：只下发实际给出的字段", () => {
  assert.deepEqual(buildDownloadsPayload({ _sessionId: "s" }), {});
  assert.deepEqual(buildDownloadsPayload({ limit: 5, clear: true, cancel: " d2 ", _sessionId: "s" }), {
    limit: 5,
    clear: true,
    cancel: "d2",
  });
  // 空 cancel / clear=false 都不下发
  assert.deepEqual(buildDownloadsPayload({ cancel: "  ", clear: false }), {});
  // --delete：去空白后下发；空串不下发（交给 shell 默认）
  assert.deepEqual(buildDownloadsPayload({ delete: " dl-3 " }), { delete: "dl-3" });
  assert.deepEqual(buildDownloadsPayload({ delete: "   " }), {});
  // download_dir：给了 --set（含空串）就下发，交给 shell 报错
  assert.deepEqual(buildDownloadDirPayload({ _sessionId: "s" }), {});
  assert.deepEqual(buildDownloadDirPayload({ set: "D:\\tmp\\dl" }), { set: "D:\\tmp\\dl" });
  assert.deepEqual(buildDownloadDirPayload({ set: "" }), { set: "" });
});

test("--limit 校验：只接受正整数", () => {
  assert.equal(downloadsLimitIssue(undefined), null);
  assert.equal(downloadsLimitIssue(5), null);
  assert.equal(downloadsLimitIssue("5"), null);
  assert.equal(downloadsLimitIssue(0), "invalid");
  assert.equal(downloadsLimitIssue(-3), "invalid");
  assert.equal(downloadsLimitIssue(2.5), "invalid");
});

test("分组输出：组标题「目录尾段 (n)」，行内 文件名/大小/状态/host", () => {
  const text = formatDownloads(
    {
      dir: "D:\\tmp\\dl",
      groups: [
        {
          dir: "D:\\tmp\\dl",
          items: [
            item({ id: "d1", filename: "b.zip", totalBytes: 45 * 1024 * 1024, receivedBytes: 12 * 1024 * 1024 }),
            item({ id: "d2", filename: "a.iso", url: "https://cdn.example.com/x", totalBytes: 0, receivedBytes: 3 * 1024 * 1024 }),
          ],
        },
        {
          dir: "/tmp/proj",
          items: [
            item({ id: "d3", filename: "c.pdf", url: "https://example.com/c.pdf", state: "completed", totalBytes: 700 * 1024, receivedBytes: 700 * 1024 }),
          ],
        },
      ],
    },
    LABELS,
  );
  assert.equal(
    text,
    [
      "dl (2)",
      "  b.zip  12MB/45MB  downloading 27%  files.example.com",
      "  a.iso  3MB  downloading unknown  cdn.example.com",
      "proj (1)",
      "  c.pdf  700KB/700KB  completed  example.com",
    ].join("\n"),
  );
});

test("行内补充：未知总量不假装百分比，失败附原因，取消不带进度", () => {
  const unknown = formatDownloadRow(item({ totalBytes: 0, receivedBytes: 512 }), LABELS);
  assert.match(unknown, /downloading unknown  files\.example\.com$/);
  const failed = formatDownloadRow(item({ state: "failed", error: "network error" }), LABELS);
  assert.equal(failed, "a.zip  0B  failed  files.example.com — network error");
  const canceled = formatDownloadRow(item({ state: "canceled" }), LABELS);
  assert.equal(canceled, "a.zip  0B  canceled  files.example.com");
});

test("空列表与目录尾段", () => {
  assert.equal(formatDownloads(undefined, LABELS), "(no downloads)");
  assert.equal(formatDownloads({ groups: [] }, LABELS), "(no downloads)");
  assert.equal(dirTail("D:\\tmp\\dl\\"), "dl");
  assert.equal(dirTail("/tmp/proj"), "proj");
  assert.equal(dirTail("/"), "/");
  assert.equal(dirTail("C:\\"), "C:");
});

test("错误透传：取消与切换目录都把 shell 原文带回模型", () => {
  const cancel = tf("cli.err.downloads_cancel_failed", { id: "d9", msg: "download is not in progress" }, "zh");
  assert.match(cancel, /d9/);
  assert.match(cancel, /download is not in progress/);
  const setErr = tf("cli.err.download_dir_set_failed", { msg: "refusing to use a filesystem root as download directory" }, "en");
  assert.match(setErr, /refusing to use a filesystem root as download directory/);
});

test("i18n zh/en 键 parity（含下载相关键）", () => {
  const keys = (locale) => Object.keys(locale.tool).sort();
  assert.deepEqual(keys(zh), keys(en));
  const downloadKeys = keys(en).filter((k) => /^cli\.(cmd|out|err|note)\.download/.test(k));
  assert.ok(downloadKeys.includes("cli.cmd.downloads"));
  assert.ok(downloadKeys.includes("cli.cmd.download_dir"));
  for (const k of downloadKeys) assert.ok(zh.tool[k], `zh 缺少 ${k}`);
});

test("--delete 输出文案：成功带文件名，文件未删时带原因（zh/en）", () => {
  assert.equal(tf("cli.out.downloads_deleted", { name: "a.zip" }, "zh"), "已删除 a.zip（记录与文件）");
  assert.match(tf("cli.out.downloads_deleted", { name: "a.zip" }, "en"), /a\.zip .*record \+ file/);
  assert.match(
    tf("cli.out.downloads_deleted_keep_file", { name: "a.zip", msg: "file not found" }, "zh"),
    /a\.zip.*file not found/,
  );
  assert.match(tf("cli.err.downloads_delete_failed", { id: "dl-1", msg: "正在下载，请先取消" }, "zh"), /dl-1/);
  assert.match(tf("cli.err.downloads_delete_failed", { id: "dl-1", msg: "boom" }, "en"), /Failed to delete dl-1: boom/);
});
