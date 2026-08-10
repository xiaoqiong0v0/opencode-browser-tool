/**
 * 浏览器 lazy 下载器:运行时检查本地是否已安装,未安装则按镜像配置下载
 * 镜像规则: {mirror}/builds/cft/{browserVersion}/{platform}/{file}(CFT)
 *           {mirror}/builds/{name}/{revision}/{file}(旧格式)
 * npmmirror 需在 path 前补 /binaries/playwright
 */
import { existsSync, mkdirSync, writeFileSync, readFileSync, rmSync } from "fs";
import { resolve, join, dirname } from "path";
import AdmZip from "adm-zip";
import { BROWSER_MANIFEST, CDN_MIRRORS } from "./manifest.generated.js";

const COMPLETE_MARKER = "INSTALLATION_COMPLETE";

export interface DownloadOptions {
  browsersPath: string;
  mirror?: string;
  onProgress?: (percent: number) => void;
}

function platformKey(): "win64" | "linuxX64" {
  return process.platform === "win32" ? "win64" : "linuxX64";
}

/** 拼接下载 URL:处理 npmmirror 前缀差异 */
export function buildDownloadUrl(entry: any, platform: "win64" | "linuxX64", mirror: string): string {
  const dl = entry.download[platform];
  if (!dl) throw new Error(`Browser ${entry.name} has no download for ${platform}`);
  const path = dl.path
    .replace("{browserVersion}", entry.browserVersion)
    .replace("{revision}", entry.revision);
  const isNpmMirror = /npmmirror\.com/.test(mirror);
  const prefix = isNpmMirror ? "/binaries/playwright" : "";
  return `${mirror}${prefix}/${path}`;
}

/** 本地浏览器目录(与 Playwright 目录结构一致: name-revision ) */
export function browserDir(browsersPath: string, name: string): string {
  const entry = BROWSER_MANIFEST[name];
  if (!entry) throw new Error(`Unknown browser: ${name}`);
  return resolve(browsersPath, `${name}-${entry.revision}`);
}

/** 是否已安装(存在完成标记) */
export function isInstalled(browsersPath: string, name: string): boolean {
  return existsSync(join(browserDir(browsersPath, name), COMPLETE_MARKER));
}

/** 下载并解压浏览器到 browsersPath */
export async function downloadBrowser(name: string, opts: DownloadOptions): Promise<void> {
  const entry = BROWSER_MANIFEST[name];
  if (!entry) throw new Error(`Unknown browser: ${name}`);
  const dir = browserDir(opts.browsersPath, name);
  if (isInstalled(opts.browsersPath, name)) return;

  const platform = platformKey();
  const mirrors = opts.mirror ? [opts.mirror] : entry.download[platform]?.mirrors || CDN_MIRRORS;
  if (!mirrors.length) throw new Error(`No mirror for ${name} on ${platform}`);

  mkdirSync(dir, { recursive: true });
  const zipPath = join(dir, "browser.zip");

  let lastErr: Error | null = null;
  for (const mirror of mirrors) {
    const url = buildDownloadUrl(entry, platform, mirror);
    try {
      await downloadFile(url, zipPath, opts.onProgress);
      lastErr = null;
      break;
    } catch (e: any) {
      lastErr = e;
      try { rmSync(zipPath, { force: true }); } catch {}
    }
  }
  if (lastErr) {
    try { rmSync(dir, { recursive: true, force: true }); } catch {}
    throw new Error(`Download ${name} failed: ${lastErr.message}`);
  }

  // 解压(adm-zip 处理嵌套目录:Playwright zip 内含 chrome-win64/ 顶层目录)
  try {
    const zip = new AdmZip(zipPath);
    zip.extractAllTo(dir, true);
  } catch (e: any) {
    try { rmSync(dir, { recursive: true, force: true }); } catch {}
    throw new Error(`Extract ${name} failed: ${e.message}`);
  }
  try { rmSync(zipPath, { force: true }); } catch {}

  // 写入完成标记
  writeFileSync(join(dir, COMPLETE_MARKER), "");
}

async function downloadFile(url: string, dest: string, onProgress?: (p: number) => void): Promise<void> {
  const res = await fetch(url);
  if (!res.ok) throw new Error(`HTTP ${res.status} for ${url}`);
  const total = Number(res.headers.get("content-length")) || 0;
  const buf = Buffer.from(await res.arrayBuffer());
  if (total && onProgress) onProgress(100);
  writeFileSync(dest, buf);
}
