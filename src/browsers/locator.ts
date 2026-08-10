/**
 * 浏览器可执行文件定位:根据平台在下载目录中查找
 */
import { existsSync } from "fs";
import { resolve, join } from "path";
import { browserDir } from "./downloader.js";

/** 浏览器可执行文件相对路径(解压后顶层目录结构固定) */
function exeRelPath(name: string): string[] {
  if (name === "chromium") {
    return process.platform === "win32"
      ? ["chrome-win64", "chrome.exe"]
      : ["chrome-linux64", "chrome"];
  }
  if (name === "firefox") {
    return process.platform === "win32"
      ? ["firefox", "firefox.exe"]
      : ["firefox", "firefox"];
  }
  return [];
}

/** 查找浏览器可执行文件(含新/旧目录结构兼容) */
export function findExecutable(browsersPath: string, name: string): string | null {
  const dir = browserDir(browsersPath, name);
  const rel = exeRelPath(name);
  if (!rel.length) return null;
  const direct = resolve(dir, ...rel);
  if (existsSync(direct)) return direct;
  // 兼容 zip 顶层目录名变化:扫描一级子目录找可执行文件
  const { readdirSync } = require("fs") as typeof import("fs");
  try {
    for (const sub of readdirSync(dir)) {
      const cand = resolve(dir, sub, rel[rel.length - 1]);
      if (existsSync(cand)) return cand;
    }
  } catch {}
  return null;
}
