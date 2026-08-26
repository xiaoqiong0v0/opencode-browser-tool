// 主题应用:通过 html[data-theme] 控制 CSS 变量,auto 跟随系统深浅色
// 模式持久化到 localStorage(tauri 本地页面同源共享),并监听 Rust 广播事件
import { listen } from "@tauri-apps/api/event";

const STORAGE_KEY = "bt-theme";

let current = "auto";

/** 应用主题模式:auto 时按系统偏好解析为 light/dark,否则直接使用 */
export function applyTheme(mode: string): void {
  current = mode;
  const html = document.documentElement;
  if (mode === "auto") {
    html.dataset.theme = window.matchMedia("(prefers-color-scheme: light)").matches ? "light" : "dark";
  } else {
    html.dataset.theme = mode;
  }
}

/** 读取已保存的主题模式(默认 auto) */
export function getTheme(): string {
  try {
    return localStorage.getItem(STORAGE_KEY) || "auto";
  } catch {
    return "auto";
  }
}

/** 保存主题模式 */
export function saveTheme(mode: string): void {
  try {
    localStorage.setItem(STORAGE_KEY, mode);
  } catch {
    // localStorage 不可用时忽略(仍可应用当前会话)
  }
}

/** 初始化主题:应用已保存模式 + 系统变化实时跟随 + 监听 Rust 广播 */
export async function initTheme(): Promise<void> {
  applyTheme(getTheme());
  window
    .matchMedia("(prefers-color-scheme: light)")
    .addEventListener("change", () => applyTheme(current));
  await listen("theme-changed", (e) => {
    const mode = String((e as { payload: unknown }).payload);
    applyTheme(mode);
  });
}
