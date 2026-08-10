# opencode-browser-tool

opencode 插件：WebView2 外壳渲染浏览器 + 零注入视觉元素选取面板，提供 30+ 个浏览器自动化工具。

> 架构演进：V1-V3 使用 Playwright 注入式面板（存在弹框类元素无法批注的问题）；V4 改为 Rust Shell + 原始调试协议（页面零注入，弹框类元素可正常批注）。

## 安装

```jsonc
// opencode.json
{
  "plugin": ["@xiaoqiong0v0/opencode-browser-tool"]
}
```

## 配置

配置文件：`~/.config/opencode/browser-tool.jsonc`（首次运行自动生成）

```jsonc
{
  "panelLang": "zh",
  "toolLang": "en",
  "sessionIsolation": false,
  "browserType": "chromium",
  "browsersPath": "D:/browsers",
  "playwrightMirror": "https://cdn.npmmirror.com/binaries/playwright/",
  "webviewDataMode": "isolated"
}
```

项目级配置放 `<项目根目录>/.opencode/browser-tool.jsonc`，会覆盖全局。

环境变量（优先级最高）：
- `PW_LANG` — 同时设置 panelLang 和 toolLang
- `PW_BROWSERS_PATH` — 浏览器缓存目录
- `PW_SHELL_PATH` — 指定 Rust shell 二进制路径（开发用）

## 架构（V4）

```
opencode（Bun）
  └─ spawn("pw-shell", [--browsers-path ...]) → HTTP 127.0.0.1:PORT
       └─ Rust Shell（WebView2 渲染引擎直绘 + 覆盖层自绘）
            └─ 页面零注入，弹框类元素可正常批注
```

- 浏览器渲染引擎（WebView2，Windows 自带）直接绘制到插件窗口，页面无任何注入
- 覆盖层（自绘）显示高亮框、批注标记
- 所有工具调用通过 `POST http://127.0.0.1:PORT/api/{command}` 通信
- 支持共享 Edge 用户数据（继承登录态）与独立数据两种模式

## 工具

| 类别 | 工具 |
|------|------|
| **导航** | `pw_navigate`, `pw_go_back`, `pw_go_forward`, `pw_reload`, `pw_close` |
| **交互** | `pw_click`, `pw_fill`, `pw_clear`, `pw_select`, `pw_hover`, `pw_drag`, `pw_press_key`, `pw_upload_file` |
| **标签页** | `pw_list_tabs`, `pw_switch_tab`, `pw_new_tab`, `pw_close_tab`, `pw_click_and_switch_tab` |
| **信息** | `pw_screenshot`, `pw_evaluate`, `pw_get_visible_text`, `pw_get_visible_html`, `pw_console_logs`, `pw_get_browser_status`, `pw_get_element_state`, `pw_get_dropdown_options`, `pw_list_records` |
| **滚动/等待** | `pw_scroll`, `pw_scroll_to_element`, `pw_wait_for_selector`, `pw_resize` |
| **网络** | `pw_expect_response`, `pw_assert_response` |
| **其他** | `pw_save_as_pdf`, `pw_custom_user_agent`, `pw_select_user_agent`, `pw_show_notification`, `pw_read_record_content` |

## 批注面板

- **批注模式** — 悬停高亮元素（覆盖层自绘），点击添加批注；引擎层拦截，弹框/下拉类元素不失焦
- **截图工具** — 框选区域，添加说明
- **发送全部** → 数据推送到 opencode 对话

## 多会话

父子会话共享一个浏览器。开启 `sessionIsolation: true` 后，每个 opencode 会话有独立上下文（cookies/存储隔离）。

## 发布

```bash
cd rust && cargo build --release
npm publish
```

只发布 `dist/` 和 `bin/`（`files: ["dist", "bin"]`）。

## 开发

```bash
npm install
cd rust && cargo build
npm run build
```

## 文档

- [docs/architecture.md](./docs/architecture.md) — 当前架构（V3 注入式，V4 迁移中）
- [docs/v4-shell-architecture.md](./docs/v4-shell-architecture.md) — V4 外壳 + 原始调试浏览器方案
- [docs/implementation.md](./docs/implementation.md) — 实施状态
