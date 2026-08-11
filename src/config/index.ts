import { readFileSync, existsSync, writeFileSync, mkdirSync } from "fs";
import { resolve, dirname } from "path";
import { homedir } from "os";

const PLUGIN_NAME = "opencode-browser-tool";
const CONFIG_FILE = "playwright-tool.jsonc";

export type Lang = "en" | "zh";

export type BrowserType = "chromium" | "firefox" | "webkit";

export interface PluginConfig {
  browserType?: BrowserType;
  browsersPath?: string;
  nodePath?: string;
  panelLang: Lang;
  toolLang: Lang;
  disabledTools?: string[];
  playwrightMirror?: string;
  sessionIsolation?: boolean;
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

  // 浏览器下载镜像（可选，默认用 npmmirror 国内镜像）
  // 置空 "playwrightMirror": "" 使用 Playwright 官方源
  // "playwrightMirror": "https://npmmirror.com/mirrors/playwright/",

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

