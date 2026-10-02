//! 二进制分发:从 GitHub Release 下载 bt-shell 到插件数据目录并校验
//!
//! 目录布局(固定路径,就地覆盖;不按插件版本建目录,避免累积与无效重下):
//!   `<pluginDataDir>/<triple>/bt-shell[.exe]`        可执行文件
//!   `<pluginDataDir>/<triple>/bt-shell.version.json` 来源旁文件(tag/asset/sha256/size/…)
//!
//! 设计要点:
//! - 幂等:目标存在 **且** sha256 与"当前插件版本对应 release 的 SHA256SUMS"一致 → 跳过;
//!   **每次都拉 SHA256SUMS 校验**(取不到即失败,不跳过校验),但不会因插件版本变化无条件重下
//! - 原子:下载到 `<target>.part`,校验通过后 `rename` 替换到最终路径
//! - 并发:同进程共享 Promise;跨进程用 `<target>.lock`(未过期则等待复用,避免重复下载)
//! - 有界:`ensureBinary(timeoutMs)` 等待至就绪/超时,不无限等待
//! - 迁移:启动清理旧布局的版本目录(`<pluginDataDir>/<x.y.z>/`),可移动一份合法资产省一次下载
//! - 代理:优先用系统 `curl`(自动遵循 HTTPS_PROXY/HTTP_PROXY),不可用时回退 `fetch`
//!
//! 环境变量:BT_SHELL_PATH / BT_SHELL_NO_DOWNLOAD / BT_SHELL_DOWNLOAD_BASE / BT_SHELL_VERSION

import { spawn, spawnSync } from "child_process";
import { createHash } from "crypto";
import {
  chmodSync,
  closeSync,
  createWriteStream,
  existsSync,
  mkdirSync,
  openSync,
  readdirSync,
  readFileSync,
  readSync,
  renameSync,
  rmSync,
  statSync,
  unlinkSync,
  writeFileSync,
} from "fs";
import { dirname, resolve } from "path";
import { fileURLToPath, pathToFileURL } from "url";
import { getPluginDataDir } from "./config/index.js";

const __dirname = dirname(fileURLToPath(import.meta.url));

/** 默认下载根(不含 `/v<version>/<asset>`),可用 BT_SHELL_DOWNLOAD_BASE 覆盖 */
const DEFAULT_DOWNLOAD_BASE = "https://github.com/xiaoqiong0v0/opencode-browser-tool/releases/download";
/** 跨进程锁过期时间(ms):超过则视为陈旧锁可接管 */
const LOCK_STALE_MS = 10 * 60 * 1000;
/** 等待其它进程下载完成的上限(ms) */
const WAIT_OTHER_PROCESS_MS = 60 * 1000;
/** 版本旁文件名 */
const SIDECAR_NAME = "bt-shell.version.json";
/** 旧布局版本目录名(如 `1.2.0`;仅删符合该形态的目录) */
const LEGACY_VERSION_DIR_RE = /^\d+\.\d+\.\d+([-+][0-9A-Za-z.-]+)?$/;

export type BinaryState = "idle" | "ready" | "downloading" | "failed" | "disabled" | "unsupported";

export interface BinaryStatus {
  state: BinaryState;
  /** 下载进度百分比(0-100;总大小未知时为 undefined) */
  progress?: number;
  /** 失败/禁用/不支持的可读原因 */
  reason?: string;
  /** 二进制最终路径(就绪时存在) */
  path?: string;
}

/** 来源旁文件内容 */
interface VersionInfo {
  /** 二进制发布 tag(推导后,如 `v1.2.0`) */
  tag: string;
  /** 插件自身版本(便于追溯"哪个插件版本下的二进制") */
  pluginVersion: string;
  asset: string;
  sha256: string;
  size: number;
  downloadedAt: string;
  sourceUrl: string;
}

interface PlatformInfo {
  triple: string;
  assetName: string;
}

/** 当前平台 → Release 资产信息;不支持的平台返回 null */
export function platformInfo(): PlatformInfo | null {
  const { platform, arch } = process;
  if (platform === "win32" && arch === "x64") {
    return { triple: "x86_64-pc-windows-msvc", assetName: "bt-shell-x86_64-pc-windows-msvc.exe" };
  }
  if (platform === "linux" && arch === "x64") {
    return { triple: "x86_64-unknown-linux-gnu", assetName: "bt-shell-x86_64-unknown-linux-gnu" };
  }
  return null;
}

/** 插件自身版本(读包元数据;不受 BT_SHELL_VERSION 影响) */
export function pluginVersion(): string {
  try {
    const pkg = JSON.parse(readFileSync(resolve(__dirname, "..", "package.json"), "utf-8"));
    if (pkg?.version) return String(pkg.version);
  } catch {
    // 忽略:回退占位版本
  }
  return "0.0.0";
}

/**
 * 二进制发布版本:取插件版本的 `<major>.<minor>.0`(忽略预发布/构建元数据)。
 *
 * 版本策略:二进制产物**只挂在 `x.Y.0` 的 Release 上** —— 动了 Rust 升中间位(minor)并发新 exe;
 * 只改插件升末位(patch),沿用同一二进制 tag。故插件需由自身版本推导二进制 tag:
 * `1.2.1 → 1.2.0`、`1.3.0 → 1.3.0`、`1.3.0-beta.1 → 1.3.0`、`2.0.5 → 2.0.0`。
 */
export function binaryReleaseVersion(version: string): string {
  const m = String(version).trim().match(/^v?(\d+)\.(\d+)(?:\.\d+)?/);
  if (!m) return "0.0.0";
  return `${m[1]}.${m[2]}.0`;
}

/**
 * 实际要拉取的二进制发布版本:
 * - `BT_SHELL_VERSION` 显式指定时**原样使用**(不做归 0 推导;形如 `1.2.0`,不带 `v`)
 * - 否则由插件版本推导(`binaryReleaseVersion`)
 */
export function binaryVersion(): string {
  const env = process.env.BT_SHELL_VERSION?.trim();
  if (env) return env;
  return binaryReleaseVersion(pluginVersion());
}

/** 二进制发布 tag:`v<binaryVersion>` */
export function binaryTag(): string {
  return `v${binaryVersion()}`;
}

/** Release 下载根(含推导后的二进制版本):`<base>/v<major>.<minor>.0` */
function releaseBase(): string {
  const base = (process.env.BT_SHELL_DOWNLOAD_BASE?.trim() || DEFAULT_DOWNLOAD_BASE).replace(/\/+$/, "");
  return `${base}/v${binaryVersion()}`;
}

/** 本地可执行文件名(固定,不带 triple/版本) */
function localBinaryName(): string {
  return process.platform === "win32" ? "bt-shell.exe" : "bt-shell";
}

/** 本地 triple 目录名(不支持平台时用 `${platform}-${arch}` 占位) */
function localTriple(): string {
  return platformInfo()?.triple ?? `${process.platform}-${process.arch}`;
}

/** 固定缓存路径:`<pluginDataDir>/<triple>/bt-shell[.exe]` */
export function cachedBinaryPath(): string {
  return resolve(getPluginDataDir(), localTriple(), localBinaryName());
}

/** 版本旁文件路径(与可执行文件同目录) */
export function cachedSidecarPath(): string {
  return resolve(dirname(cachedBinaryPath()), SIDECAR_NAME);
}

/** 供外部(工具提示)使用:二进制的三元组/文件名/下载 URL/缓存路径 */
export function getBinaryInfo() {
  const info = platformInfo();
  if (!info) return null;
  const path = cachedBinaryPath();
  return {
    pluginVersion: pluginVersion(),
    version: binaryVersion(),
    tag: binaryTag(),
    triple: info.triple,
    assetName: info.assetName,
    url: `${releaseBase()}/${info.assetName}`,
    sumsUrl: `${releaseBase()}/SHA256SUMS`,
    path,
    sidecarPath: cachedSidecarPath(),
  };
}

// ---- 状态机 ----
let current: BinaryStatus = { state: "idle", path: undefined };
/** 同进程内共享的下载 Promise(并发只下载一次) */
let inflight: Promise<void> | null = null;

export function getBinaryStatus(): BinaryStatus {
  return { ...current };
}

function setStatus(next: BinaryStatus): void {
  current = next;
}

/** 触发后台下载(幂等;fire-and-forget,失败不抛出) */
export function startDownload(): void {
  if (inflight) return;
  if (current.state === "ready" || current.state === "disabled" || current.state === "unsupported") return;
  inflight = doDownload()
    .catch(() => {
      // 失败原因已写入状态;这里吞掉,避免影响调用方(opencode 启动)
    })
    .finally(() => {
      inflight = null;
    });
}

/** 等待二进制就绪或超时(不无限等待);超时返回当前状态 */
export async function ensureBinary(timeoutMs = 20000): Promise<BinaryStatus> {
  startDownload();
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    const s = getBinaryStatus();
    if (s.state !== "idle" && s.state !== "downloading") return s;
    await sleep(200);
  }
  return getBinaryStatus();
}

/** 就绪状态的可读提示文案(供工具在未就绪时返回) */
export function describeBinaryStatus(s: BinaryStatus): string {
  switch (s.state) {
    case "ready":
      return s.path ?? "ready";
    case "downloading":
      return `bt-shell 正在下载（${s.progress ?? 0}%）… 稍后重试`;
    case "failed":
      return `bt-shell 下载失败：${s.reason ?? "未知原因"}；可用 BT_SHELL_DOWNLOAD_BASE 换源、或 BT_SHELL_PATH 手动指定二进制`;
    case "disabled":
      return `bt-shell 自动下载已禁用（BT_SHELL_NO_DOWNLOAD=1）；请用 BT_SHELL_PATH 指定二进制，或把它放到 ${s.path ?? cachedBinaryPath()}`;
    case "unsupported":
      return `bt-shell 暂不支持当前平台：${s.reason ?? `${process.platform}/${process.arch}`}`;
    default:
      return "bt-shell 尚未就绪，请稍后重试";
  }
}

// ---- 下载主流程 ----
async function doDownload(): Promise<void> {
  const path = cachedBinaryPath();
  if (process.env.BT_SHELL_NO_DOWNLOAD === "1") {
    setStatus({ state: "disabled", reason: "BT_SHELL_NO_DOWNLOAD=1", path });
    return;
  }
  const info = platformInfo();
  if (!info) {
    setStatus({ state: "unsupported", reason: `${process.platform}/${process.arch}`, path });
    return;
  }

  const finalPath = path;
  const partPath = `${finalPath}.part`;
  const lockPath = `${finalPath}.lock`;
  const tag = binaryTag();
  const sourceUrl = `${releaseBase()}/${info.assetName}`;
  mkdirSync(dirname(finalPath), { recursive: true });
  setStatus({ state: "downloading", progress: 0, path: finalPath });

  // 1) 每次都拉 SHA256SUMS 做校验(取不到即失败,不跳过校验)
  let sumsText: string;
  try {
    sumsText = await fetchText(`${releaseBase()}/SHA256SUMS`);
  } catch (e) {
    const hint = notFoundHint(tag, e);
    setStatus({
      state: "failed",
      reason: `无法获取 SHA256SUMS（不跳过校验）：${errMsg(e)}${hint ? `；${hint}` : ""}`,
      path: finalPath,
    });
    return;
  }
  const expected = parseSums(sumsText, info.assetName);
  if (!expected) {
    setStatus({ state: "failed", reason: `SHA256SUMS 中未找到 ${info.assetName}`, path: finalPath });
    return;
  }

  // 2) 迁移:清理旧布局版本目录,必要时从旧目录移动合法资产(同盘 rename)
  migrateLegacyDirs(info, finalPath, expected, sourceUrl);

  // 3) 幂等:目标存在且 sha 与当前 release 期望一致 → 跳过(不重下)
  if (existsSync(finalPath) && sha256File(finalPath) === expected) {
    ensureSidecar(finalPath, info, expected, sourceUrl);
    setStatus({ state: "ready", path: finalPath });
    return;
  }

  // 4) 跨进程锁:抢不到则等其它进程下载完成
  if (!acquireLock(lockPath)) {
    const ok = await waitForValidFile(finalPath, expected, WAIT_OTHER_PROCESS_MS);
    if (ok) {
      ensureSidecar(finalPath, info, expected, sourceUrl);
      setStatus({ state: "ready", path: finalPath });
    } else {
      setStatus({ state: "failed", reason: "等待其它进程下载超时（可删除 .lock 后重试）", path: finalPath });
    }
    return;
  }

  try {
    try {
      if (existsSync(partPath)) unlinkSync(partPath);
    } catch {}
    await downloadTo(sourceUrl, partPath, (p) => setStatus({ state: "downloading", progress: p, path: finalPath }));
    const got = sha256File(partPath);
    if (got !== expected) {
      safeUnlink(partPath);
      setStatus({
        state: "failed",
        reason: `sha256 校验失败：期望 ${expected.slice(0, 12)}… 实际 ${got.slice(0, 12)}…`,
        path: finalPath,
      });
      return;
    }
    renameSync(partPath, finalPath); // 原子替换
    if (process.platform !== "win32") {
      try {
        chmodSync(finalPath, 0o755);
      } catch {}
    }
    ensureSidecar(finalPath, info, expected, sourceUrl);
    setStatus({ state: "ready", path: finalPath });
  } catch (e) {
    safeUnlink(partPath);
    const hint = notFoundHint(tag, e);
    setStatus({ state: "failed", reason: `${errMsg(e)}${hint ? `；${hint}` : ""}`, path: finalPath });
  } finally {
    releaseLock(lockPath);
  }
}

// ---- 迁移:旧布局 `<pluginDataDir>/<x.y.z>/<triple>/<asset>` → 新固定路径 ----
/**
 * 清理旧布局版本目录(仅匹配 `<x.y.z>` 形态,其它一律不碰)。
 * - dry-run:先把清单打印到日志
 * - 若新路径无合法文件,优先从旧目录"移动"一份 sha 匹配的资产(同盘 rename,省一次下载)
 * - 随后删除全部旧版本目录
 */
function migrateLegacyDirs(info: PlatformInfo, finalPath: string, expected: string, sourceUrl: string): void {
  const dataDir = getPluginDataDir();
  let entries;
  try {
    entries = readdirSync(dataDir, { withFileTypes: true });
  } catch {
    return;
  }
  const legacyDirs = entries
    .filter((e) => e.isDirectory() && LEGACY_VERSION_DIR_RE.test(e.name))
    .map((e) => resolve(dataDir, e.name));
  if (legacyDirs.length === 0) return;

  console.log(`[bt-shell][migrate] dry-run 旧版本目录清单: ${legacyDirs.join(", ")}`);

  const newValid = existsSync(finalPath) && sha256File(finalPath) === expected;
  if (!newValid) {
    for (const dir of legacyDirs) {
      const cand = resolve(dir, info.triple, info.assetName);
      try {
        if (existsSync(cand) && sha256File(cand) === expected) {
          mkdirSync(dirname(finalPath), { recursive: true });
          renameSync(cand, finalPath); // 同盘 rename
          console.log(`[bt-shell][migrate] moved ${cand} -> ${finalPath}`);
          break;
        }
      } catch {
        // 候选不可用则继续尝试下一个
      }
    }
  } else {
    console.log(`[bt-shell][migrate] 新路径已有合法文件,保留 ${finalPath}`);
  }

  for (const dir of legacyDirs) {
    try {
      rmSync(dir, { recursive: true, force: true });
      console.log(`[bt-shell][migrate] removed ${dir}`);
    } catch (e) {
      console.log(`[bt-shell][migrate] remove failed ${dir}: ${errMsg(e)}`);
    }
  }

  if (existsSync(finalPath) && sha256File(finalPath) === expected) {
    ensureSidecar(finalPath, info, expected, sourceUrl, "migrated");
  }
}

/** 写版本旁文件;已存在且 sha 一致则不动(避免无谓改写 mtime) */
function ensureSidecar(
  finalPath: string,
  info: PlatformInfo,
  sha256: string,
  sourceUrl: string,
  extra?: string,
): void {
  const sidecarPath = resolve(dirname(finalPath), SIDECAR_NAME);
  try {
    if (existsSync(sidecarPath)) {
      const prev = JSON.parse(readFileSync(sidecarPath, "utf-8"));
      if (
        prev?.sha256 === sha256 &&
        prev?.asset === info.assetName &&
        prev?.tag === binaryTag()
      ) {
        return;
      }
    }
    const version: VersionInfo = {
      tag: binaryTag(),
      pluginVersion: pluginVersion(),
      asset: info.assetName,
      sha256,
      size: statSync(finalPath).size,
      downloadedAt: new Date().toISOString(),
      sourceUrl,
    };
    writeFileSync(sidecarPath, JSON.stringify(version, null, 2), "utf-8");
    console.log(`[bt-shell] 写入版本旁文件 ${sidecarPath}${extra ? ` (${extra})` : ""}`);
  } catch (e) {
    console.log(`[bt-shell] 写入版本旁文件失败: ${errMsg(e)}`);
  }
}

// ---- 下载实现:curl 优先(自动遵循系统代理),回退 fetch ----
let curlChecked = false;
let curlOk = false;
function curlAvailable(): boolean {
  if (!curlChecked) {
    curlChecked = true;
    try {
      curlOk = spawnSync("curl", ["--version"], { stdio: "ignore" }).status === 0;
    } catch {
      curlOk = false;
    }
  }
  return curlOk;
}

async function downloadTo(url: string, dest: string, onProgress: (p?: number) => void): Promise<void> {
  if (curlAvailable()) return downloadWithCurl(url, dest, onProgress);
  return downloadWithFetch(url, dest, onProgress);
}

/** 用系统 curl 下载(curl 自动读取 HTTPS_PROXY/HTTP_PROXY/NO_PROXY 并跟随重定向) */
async function downloadWithCurl(url: string, dest: string, onProgress: (p?: number) => void): Promise<void> {
  const total = curlContentLength(url);
  await new Promise<void>((resolvePromise, reject) => {
    const child = spawn("curl", ["-L", "--fail", "--silent", "--show-error", "-o", dest, url], {
      stdio: ["ignore", "ignore", "pipe"],
    });
    let stderr = "";
    child.stderr?.on("data", (d) => {
      stderr += d.toString();
    });
    const timer = setInterval(() => {
      try {
        const size = statSync(dest).size;
        onProgress(total ? Math.min(100, Math.floor((size * 100) / total)) : undefined);
      } catch {
        // .part 尚未创建
      }
    }, 200);
    child.on("error", (e) => {
      clearInterval(timer);
      reject(e);
    });
    child.on("close", (code) => {
      clearInterval(timer);
      if (code === 0) resolvePromise();
      else reject(new Error(`curl exit ${code}: ${stderr.trim() || "下载失败"}`));
    });
  });
}

/** HEAD 取 Content-Length(取不到返回 undefined;进度仍可降级为未知) */
function curlContentLength(url: string): number | undefined {
  try {
    const r = spawnSync("curl", ["-sIL", url], { encoding: "utf-8", maxBuffer: 8 * 1024 * 1024 });
    if (r.status !== 0 || !r.stdout) return undefined;
    let total: number | undefined;
    for (const line of r.stdout.split(/\r?\n/)) {
      const m = line.match(/^content-length:\s*(\d+)/i);
      if (m) {
        const n = Number(m[1]);
        if (n > 0) total = n;
      }
    }
    return total;
  } catch {
    return undefined;
  }
}

async function downloadWithFetch(url: string, dest: string, onProgress: (p?: number) => void): Promise<void> {
  const res = await fetch(url, { redirect: "follow" });
  if (!res.ok || !res.body) throw new Error(`HTTP ${res.status} ${res.statusText}`);
  const total = Number(res.headers.get("content-length") || 0) || undefined;
  const out = createWriteStream(dest);
  const reader = res.body.getReader();
  let received = 0;
  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      if (value) {
        received += value.length;
        out.write(Buffer.from(value));
        onProgress(total ? Math.min(100, Math.floor((received * 100) / total)) : undefined);
      }
    }
  } finally {
    await new Promise<void>((r) => out.end(() => r()));
  }
}

/** 取文本:fetch 优先(无代理时直连);失败回退 curl(代理环境) */
async function fetchText(url: string): Promise<string> {
  try {
    const res = await fetch(url, { redirect: "follow" });
    if (!res.ok) throw new Error(`HTTP ${res.status} ${res.statusText}`);
    return await res.text();
  } catch (e) {
    if (curlAvailable()) return curlText(url);
    throw e;
  }
}

function curlText(url: string): string {
  const r = spawnSync("curl", ["-sL", "--fail", url], { encoding: "utf-8", maxBuffer: 8 * 1024 * 1024 });
  if (r.status !== 0) throw new Error(`curl exit ${r.status}: ${(r.stderr || "").trim() || "获取失败"}`);
  return r.stdout || "";
}

// ---- 校验/锁/工具 ----
/** 计算文件 sha256(hex,流式读取,避免大文件占内存) */
function sha256File(path: string): string {
  const hash = createHash("sha256");
  const fd = openSync(path, "r");
  const buf = Buffer.alloc(1024 * 256);
  try {
    for (;;) {
      const n = readSync(fd, buf, 0, buf.length, null);
      if (n <= 0) break;
      hash.update(buf.subarray(0, n));
    }
  } finally {
    closeSync(fd);
  }
  return hash.digest("hex");
}

/** 从 SHA256SUMS 解析目标资产的 sha256;找不到返回 null */
function parseSums(text: string, assetName: string): string | null {
  for (const line of text.split(/\r?\n/)) {
    const m = line.trim().match(/^([0-9a-fA-F]{64})\s+\*?(.+)$/);
    if (m && m[2].trim() === assetName) return m[1].toLowerCase();
  }
  return null;
}

/** 抢占跨进程锁(原子创建);陈旧锁可接管;失败返回 false(表示其它进程在下载) */
function acquireLock(lockPath: string): boolean {
  try {
    writeFileSync(lockPath, `${process.pid} ${Date.now()}`, { flag: "wx" });
    return true;
  } catch (e: any) {
    if (e?.code !== "EEXIST") return true; // 其它错误不阻塞下载
    try {
      const age = Date.now() - statSync(lockPath).mtimeMs;
      if (age > LOCK_STALE_MS) {
        unlinkSync(lockPath);
        return acquireLock(lockPath);
      }
    } catch {
      // 锁文件刚被删除等:重试一次
      return acquireLock(lockPath);
    }
    return false;
  }
}

function releaseLock(lockPath: string): void {
  safeUnlink(lockPath);
}

async function waitForValidFile(path: string, expected: string, timeoutMs: number): Promise<boolean> {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      if (existsSync(path) && sha256File(path) === expected) return true;
    } catch {}
    await sleep(300);
  }
  return false;
}

function safeUnlink(path: string): void {
  try {
    if (existsSync(path)) unlinkSync(path);
  } catch {}
}

function errMsg(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

/** Release/资产不存在(HTTP 404 / curl exit 22)时的针对性提示 */
function notFoundHint(tag: string, e: unknown): string {
  const m = errMsg(e).toLowerCase();
  const notFound = m.includes("404") || m.includes("curl exit 22") || m.includes("not found");
  if (!notFound) return "";
  return `未找到该版本对应的 Release 资产（tag=${tag}）；若这是纯插件改动请确认二进制产物仍在 x.Y.0 的 Release 中，若动了 Rust 需先升中间版本号并新开/更新对应 Release`;
}

function sleep(ms: number): Promise<void> {
  return new Promise((r) => setTimeout(r, ms));
}

// ---- 独立运行:`node dist/binary.js [--timeout=ms]`〔便于测试〕----
const isMain = process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href;
if (isMain) {
  const arg = process.argv.find((a) => a.startsWith("--timeout="));
  const timeout = arg ? Number(arg.split("=")[1]) : 20000;
  startDownload();
  ensureBinary(Number.isFinite(timeout) ? timeout : 20000).then((s) => {
    console.log(JSON.stringify(s, null, 2));
    process.exit(s.state === "ready" || s.state === "disabled" || s.state === "unsupported" ? 0 : 1);
  });
}
