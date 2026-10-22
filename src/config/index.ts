import { readFileSync, existsSync, writeFileSync, mkdirSync } from "fs";
import { resolve, dirname, relative, isAbsolute, sep } from "path";
import { homedir } from "os";
import { tf } from "../i18n/index.js";
import { log } from "../logger.js";

const PLUGIN_NAME = "opencode-browser-tool";
const CONFIG_FILE = "browser-tool.jsonc";

/** profile 名长度上限(字符) */
const PROFILE_NAME_MAX_LEN = 64;
/** profile 名禁止出现的字符(/ \ : NUL 与 Windows 非法文件名字符) */
const PROFILE_NAME_FORBIDDEN = /[\/\\:\u0000<>"|?*]/;
/** Windows 保留设备名(大小写不敏感;含带扩展名的形式,如 CON.txt) */
const WINDOWS_RESERVED_NAMES = new Set([
  "CON", "PRN", "AUX", "NUL",
  ...Array.from({ length: 9 }, (_, i) => `COM${i + 1}`),
  ...Array.from({ length: 9 }, (_, i) => `LPT${i + 1}`),
]);

export type Lang = "en" | "zh";

// 1.9.0:移除 browserType / sessionIsolation —— 二者透传给 bt-shell 的 --browser / --session-isolation
// 从不被 Rust 解析(main.rs parse_args 静默丢弃),是**死参数**;1.9.0 起每会话独立 shell 实例成为
// 唯一行为,继续保留会误导用户。旧配置文件里若仍有这两项,applyConfigFile 会忽略(见函数注释)。

export interface PluginConfig {
  browsersPath?: string;
  nodePath?: string;
  panelLang: Lang;
  toolLang: Lang;
  disabledTools?: string[];
  /** 缓存根目录(默认 ~/.opencode/plugins-cache/opencode-browser-tool) */
  cacheDir?: string;
}

const defaults: PluginConfig = { panelLang: "en", toolLang: "en" };
let _config: PluginConfig = { ...defaults };
let _worktree: string = "";

/** 获取 opencode 全局配置目录 */
export function getOpenCodeDir(): string {
  return resolve(homedir(), ".config", "opencode");
}

/** 获取 opencode 全局数据目录 */
export function getOpenCodeDataDir(): string {
  return resolve(homedir(), ".opencode");
}

/** 获取插件数据目录 */
export function getPluginDataDir(): string {
  return resolve(getOpenCodeDataDir(), "plugins-data", PLUGIN_NAME);
}

/** 获取浏览器缓存目录 */
export function getBrowsersDir(): string {
  return _config.browsersPath || resolve(getPluginDataDir(), "browsers");
}

/** 插件缓存根目录(用户配置/导出/临时文件,默认 ~/.opencode/plugins-cache/opencode-browser-tool) */
export function getCacheDir(): string {
  return _config.cacheDir || resolve(getOpenCodeDataDir(), "plugins-cache", PLUGIN_NAME);
}

/** 多用户配置目录(user-data/<profile>,每个 profile 独立 WebView2 用户数据) */
export function getProfilesDir(): string {
  return resolve(getCacheDir(), "user-data");
}

/**
 * 校验 profile 名并返回原名(不合法则抛错,消息走 i18n `cli.err.profile_invalid_name`)
 * 规则:非空、≤64 字符、不含 / \ : NUL 及 < > " | ? *、不等于 . / ..、不以 . 开头、
 * 不是 Windows 保留设备名(CON/PRN/AUX/NUL/COM1-9/LPT1-9,大小写不敏感)。
 */
export function assertSafeProfileName(name: string): string {
  const bad = (): never => {
    throw new Error(tf("cli.err.profile_invalid_name", { name: String(name) }));
  };
  if (typeof name !== "string" || name.length === 0) bad();
  if (name.length > PROFILE_NAME_MAX_LEN) bad();
  if (PROFILE_NAME_FORBIDDEN.test(name)) bad();
  if (name === "." || name === "..") bad();
  if (name.startsWith(".")) bad();
  // Windows 保留名对带扩展名的写法同样生效(CON.txt 也不可创建)
  if (WINDOWS_RESERVED_NAMES.has(name.split(".")[0].toUpperCase())) bad();
  return name;
}

/**
 * 把 profile 名解析为绝对路径:先校验名字,再用 relative 断言结果位于 profiles 目录内
 * (非空、不以 .. 开头、非绝对路径),防止 path traversal。
 */
export function resolveProfilePath(name: string): string {
  const safe = assertSafeProfileName(name);
  const base = getProfilesDir();
  const full = resolve(base, safe);
  const rel = relative(base, full);
  if (!rel || rel === ".." || rel.startsWith(`..${sep}`) || isAbsolute(rel)) {
    throw new Error(tf("cli.err.profile_invalid_name", { name: String(name) }));
  }
  return full;
}

/**
 * 判断 child 是否在 parent **内部**(不包含 parent 自身)。
 * 用 path.relative 判定,避免 `startsWith` 的前缀误判(如 tmpX/a 对 tmp)。
 */
export function isInsideDir(child: string, parent: string): boolean {
  const rel = relative(resolve(parent), resolve(child));
  return !!rel && rel !== ".." && !rel.startsWith(`..${sep}`) && !isAbsolute(rel);
}

// ---- 会话隔离(1.9.0):每会话独立 shell 实例的数据目录 ----
// 路径: <cacheDir>/sessions/<profile>/<sid>
// 说明: 需求字面为 sessions/<sid>;此处多一层 <profile>,是为了保住 `profile --set` 的语义 ——
// 它现在的含义是「影响**后续新会话**的数据根(默认 profile)」,运行中的实例不受影响。
// 若要严格字面路径,去掉 profile 段即可(会话间隔离不受影响,只是 profile 失去作用)。

/** 会话数据根目录 */
export function getSessionsDir(): string {
  return resolve(getCacheDir(), "sessions");
}

/** 某会话的独立 user-data-dir(WebView2 profile:cookie/localStorage/缓存全隔离) */
export function getSessionDataDir(sid: string): string {
  return resolve(getSessionsDir(), getActiveProfile(), sid);
}

/** 某会话的临时产物目录(截图落盘等;不再与其它会话/全局 tmp 共享) */
export function getSessionTmpDir(sid: string): string {
  return resolve(getSessionDataDir(sid), "tmp");
}

/** 导出目录(用户从 Edge/Chrome 导出的密码 CSV / 书签 HTML) */
export function getExportsDir(): string {
  return resolve(getCacheDir(), "exports");
}

/** 临时目录(密码临时文件,用完即删) */
export function getTmpDir(): string {
  return resolve(getCacheDir(), "tmp");
}

/** active profile 状态文件 */
function activeProfileFile(): string {
  return resolve(getCacheDir(), "active-profile");
}

/** 已记录过「activeProfile 非法」告警的值(避免每次读取都刷日志) */
let _badActiveProfileLogged = "";

/** 获取当前激活配置(默认 "default";不存在或值非法时回落默认并记日志) */
export function getActiveProfile(): string {
  try {
    const v = readFileSync(activeProfileFile(), "utf-8").trim();
    if (v) {
      try {
        return assertSafeProfileName(v);
      } catch {
        // 状态文件被手工改坏时,启动路径不能因此抛错:回落 default(仅记一次日志)
        if (_badActiveProfileLogged !== v) {
          _badActiveProfileLogged = v;
          log.error(`active-profile contains invalid name (${v.length} chars); falling back to "default"`);
        }
        return "default";
      }
    }
  } catch {}
  return "default";
}

/** 持久化当前激活配置 */
export function setActiveProfile(name: string): void {
  mkdirSync(getCacheDir(), { recursive: true });
  writeFileSync(activeProfileFile(), name, "utf-8");
}

/** 加载配置：环境变量 > 项目配置 > 全局配置 > 默认值 */
export function loadConfig(worktree?: string): PluginConfig {
  _worktree = worktree || "";
  const config = { ...defaults };

  const globalPath = resolve(getOpenCodeDir(), CONFIG_FILE);
  if (!existsSync(globalPath)) generateDefaultConfig(globalPath);
  applyConfigFile(config, globalPath);

  if (_worktree) {
    applyConfigFile(config, resolve(_worktree, ".opencode", CONFIG_FILE));
  }

  applyEnv(config);

  _config = config;
  return config;
}

export function getConfig(): PluginConfig {
  return _config;
}

function applyConfigFile(config: PluginConfig, filePath: string): void {
  if (!existsSync(filePath)) return;
  try {
    const raw = readFileSync(filePath, "utf-8");
    // 剥离注释后可能存在尾逗号（如最后一项有效配置后跟注释行），需容错处理
    const json = raw
      .replace(/\/\/.*$/gm, "")
      .replace(/\/\*[\s\S]*?\*\//g, "")
      .replace(/,\s*}/g, "}");
    // 只读取已知键:旧文件里的 browserType / sessionIsolation 等已移除项会被自然忽略(兼容,不报错)
    const fileConfig: Partial<PluginConfig> = JSON.parse(json);

    if (fileConfig.browsersPath !== undefined) config.browsersPath = fileConfig.browsersPath;
    if (fileConfig.nodePath !== undefined) config.nodePath = fileConfig.nodePath;
    if (fileConfig.panelLang && isValidLang(fileConfig.panelLang)) config.panelLang = fileConfig.panelLang;
    if (fileConfig.toolLang && isValidLang(fileConfig.toolLang)) config.toolLang = fileConfig.toolLang;
    if (fileConfig.disabledTools) config.disabledTools = fileConfig.disabledTools;
    if (fileConfig.cacheDir !== undefined) config.cacheDir = fileConfig.cacheDir;
  } catch {}
}

function applyEnv(config: PluginConfig): void {
  if (process.env.BT_LANG && isValidLang(process.env.BT_LANG)) {
    config.panelLang = process.env.BT_LANG as Lang;
    config.toolLang = process.env.BT_LANG as Lang;
  }
  if (process.env.BT_PANEL_LANG && isValidLang(process.env.BT_PANEL_LANG)) {
    config.panelLang = process.env.BT_PANEL_LANG as Lang;
  }
  if (process.env.BT_TOOL_LANG && isValidLang(process.env.BT_TOOL_LANG)) {
    config.toolLang = process.env.BT_TOOL_LANG as Lang;
  }
  if (process.env.BT_BROWSERS_PATH) {
    config.browsersPath = process.env.BT_BROWSERS_PATH;
  }
  if (process.env.BT_NODE_PATH) {
    config.nodePath = process.env.BT_NODE_PATH;
  }
}

function isValidLang(lang: string): lang is Lang {
  return ["en", "zh"].includes(lang);
}

function generateDefaultConfig(filePath: string): void {
  try {
    mkdirSync(dirname(filePath), { recursive: true });
    writeFileSync(
      filePath,
      `{
  // 面板 UI 语言: "en" | "zh"（默认 "en"）
  "panelLang": "en",

  // 工具描述语言: "en" | "zh"（默认 "en"，用英文可节省 token）
  "toolLang": "en",

  // 浏览器缓存目录（可选，不设则使用插件数据目录）
  // "browsersPath": "D:/browsers/chromium",

  // 缓存根目录（可选，默认 ~/.opencode/plugins-cache/opencode-browser-tool）
  // 下设 sessions/<profile>/<sid> 每会话独立浏览器数据、user-data/<profile> 旧目录、
  // exports 导出、tmp 临时密码文件
  // "cacheDir": "D:/browser-cache",

  // 禁用的工具列表（可选，工具名不加 bt_ 前缀）
  // "disabledTools": ["close", "custom_user_agent"],

  // 1.9.0 起「每会话独立浏览器实例」为唯一行为（原 sessionIsolation 开关从未生效，已移除）；
  // browserType 同理移除（--browser 从不被 bt-shell 解析）。旧配置里若仍写着这两项会被忽略。

  // Node.js 路径（可选，不设则从 PATH 查找 node）
  // "nodePath": "C:/Program Files/nodejs/node.exe",
}
`,
      "utf-8",
    );
  } catch {}
}

