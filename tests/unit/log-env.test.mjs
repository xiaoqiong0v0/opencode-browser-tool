// 日志流配置 → spawn env 单测（纯函数；不加载配置/不启动浏览器）
import test from "node:test";
import assert from "node:assert/strict";
import {
  buildLogStreamEnv,
  bytesToMbString,
  DEFAULT_LOG_BUFFER_BYTES,
  DEFAULT_NETWORK_BURST_MS,
  DEFAULT_CONSOLE_INJECT,
} from "../../dist/config/index.js";

test("默认值：8 MiB / 2000ms / 平台默认注入开关", () => {
  const env = buildLogStreamEnv({ panelLang: "en", toolLang: "en" });
  assert.equal(env.BT_CONSOLE_BUFFER_MB, "8");
  assert.equal(env.BT_NETWORK_BUFFER_MB, "8");
  assert.equal(env.BT_NETWORK_BURST_MS, String(DEFAULT_NETWORK_BURST_MS));
  assert.equal(env.BT_CONSOLE_INJECT, DEFAULT_CONSOLE_INJECT ? "1" : "0");
  assert.equal(DEFAULT_LOG_BUFFER_BYTES, 8 * 1024 * 1024);
});

test("自定义字节预算按整数 MB 向上取整（shell 只收整数）", () => {
  const env = buildLogStreamEnv({
    panelLang: "en",
    toolLang: "en",
    consoleBufferBytes: 12 * 1024 * 1024,
    networkBufferBytes: 1.5 * 1024 * 1024,
    networkBurstWindowMs: 500,
    consoleInject: true,
  });
  assert.equal(env.BT_CONSOLE_BUFFER_MB, "12");
  assert.equal(env.BT_NETWORK_BUFFER_MB, "2");
  assert.equal(env.BT_NETWORK_BURST_MS, "500");
  assert.equal(env.BT_CONSOLE_INJECT, "1");
});

test("consoleInject=false → 0；非正字节预算回退默认", () => {
  const env = buildLogStreamEnv({ panelLang: "en", toolLang: "en", consoleInject: false, consoleBufferBytes: 0 });
  assert.equal(env.BT_CONSOLE_INJECT, "0");
  assert.equal(env.BT_CONSOLE_BUFFER_MB, "8");
});

test("bytesToMbString 向上取整到整数 MB（至少 1）", () => {
  assert.equal(bytesToMbString(8 * 1024 * 1024), "8");
  assert.equal(bytesToMbString(1024 * 1024 * 1.25), "2");
  assert.equal(bytesToMbString(1024 * 1024 * 1.000001), "2");
  assert.equal(bytesToMbString(1024), "1");
});
