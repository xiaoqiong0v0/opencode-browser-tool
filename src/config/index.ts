import { readFileSync, existsSync, writeFileSync, mkdirSync } from "fs";
import { resolve, dirname } from "path";
import { homedir } from "os";

const PLUGIN_NAME = "opencode-browser-tool";
const CONFIG_FILE = "browser-tool.jsonc";

export type Lang = "en" | "zh";

export type BrowserType = "chromium" | "firefox" | "webkit";

export interface PluginConfig {
  browserType?: BrowserType;
  browsersPath?: string;
  nodePath?: string;
  panelLang: Lang;
  toolLang: Lang;
  disabledTools?: string[];
  sessionIsolation?: boolean;
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

/** 某个配置的 WebView2 用户数据目录 */
export function getProfileDir(name: string): string {
  return resolve(getProfilesDir(), name);
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

/** 获取当前激活配置(默认 "default";不存在时回落默认) */
export function getActiveProfile(): string {
  try {
    const v = readFileSync(activeProfileFile(), "utf-8").trim();
    if (v) return v;
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
    const fileConfig: Partial<PluginConfig> = JSON.parse(json);

    if (fileConfig.browserType !== undefined) config.browserType = fileConfig.browserType;
    if (fileConfig.browsersPath !== undefined) config.browsersPath = fileConfig.browsersPath;
    if (fileConfig.nodePath !== undefined) config.nodePath = fileConfig.nodePath;
    if (fileConfig.panelLang && isValidLang(fileConfig.panelLang)) config.panelLang = fileConfig.panelLang;
    if (fileConfig.toolLang && isValidLang(fileConfig.toolLang)) config.toolLang = fileConfig.toolLang;
    if (fileConfig.disabledTools) config.disabledTools = fileConfig.disabledTools;
    if (fileConfig.sessionIsolation !== undefined) config.sessionIsolation = fileConfig.sessionIsolation;
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
  // 下设 user-data/<profile> 用户配置、exports 导出、tmp 临时密码文件
  // "cacheDir": "D:/browser-cache",

  // 禁用的工具列表（可选，工具名不加 bt_ 前缀）
  // "disabledTools": ["close", "custom_user_agent"],

  // 会话隔离：为子会话创建独立 BrowserContext（默认 false，全部共享）
  // "sessionIsolation": false,

  // Node.js 路径（可选，不设则从 PATH 查找 node）
  // "nodePath": "C:/Program Files/nodejs/node.exe",

  // 浏览器类型: "chromium" | "firefox" | "webkit"（默认 chromium）
  // "browserType": "firefox",
}
`,
      "utf-8",
    );
  } catch {}
}

