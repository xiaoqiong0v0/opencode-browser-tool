# opencode-browser-tool

opencode 插件：Tauri 多 Webview 浏览器外壳 + 零注入视觉元素选取面板，提供 40+ 个浏览器自动化工具。

> 架构演进：V1-V3 使用 Playwright 注入式面板（存在弹框类元素无法批注的问题）；V4 改为 Rust Shell + 原始调试协议；V5 迁移到 Tauri 2 多 Webview（跨平台）。

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
  "sessionIsolation": false
}
```

项目级配置放 `<项目根目录>/.opencode/browser-tool.jsonc`，会覆盖全局。

环境变量（优先级最高）：
- `BT_LANG` — 同时设置 panelLang 和 toolLang
- `BT_BROWSERS_PATH` — 浏览器缓存目录
- `BT_SHELL_PATH` — 指定 Rust shell 二进制路径（开发用）

## 架构（V5）

```
opencode（Bun）
  └─ spawn("bt-shell", [--port ...]) → HTTP 127.0.0.1:PORT
       └─ Rust Shell（Tauri 2 单窗口 + 三 Webview）
            ├─ page Webview（左侧，渲染目标网页，注入页面桥）
            ├─ overlay Webview（透明覆盖层，高亮框/批注标记）
            └─ panel Webview（右侧，控制面板）
```

- 浏览器渲染引擎（WebView2 / WebKitGTK / WKWebView，随平台）直接绘制到插件窗口
- 页面零注入残留脚本，弹框类元素可正常批注
- 所有工具调用通过 `POST http://127.0.0.1:PORT/api/{command}` 通信
- 跨平台：Windows / Linux / macOS

## 工具

| 类别 | 工具 |
|------|------|
| **导航** | `bt_navigate`, `bt_go_back`, `bt_go_forward`, `bt_reload`, `bt_close` |
| **交互** | `bt_click`, `bt_fill`, `bt_clear`, `bt_select`, `bt_hover`, `bt_drag`, `bt_press_key`, `bt_upload_file` |
| **标签页** | `bt_list_tabs`, `bt_switch_tab`, `bt_new_tab`, `bt_close_tab`, `bt_click_and_switch_tab` |
| **信息** | `bt_screenshot`, `bt_evaluate`, `bt_get_visible_text`, `bt_get_visible_html`, `bt_console_logs`, `bt_get_browser_status`, `bt_get_element_state`, `bt_get_dropdown_options`, `bt_list_records` |
| **滚动/等待** | `bt_scroll`, `bt_scroll_to_element`, `bt_wait_for_selector`, `bt_resize` |
| **网络** | `bt_expect_response`, `bt_assert_response` |
| **其他** | `bt_save_as_pdf`, `bt_custom_user_agent`, `bt_select_user_agent`, `bt_show_notification`, `bt_read_record_content` |

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

- [docs/v5-tauri-architecture.md](./docs/v5-tauri-architecture.md) — V5 当前架构
- [docs/implementation.md](./docs/implementation.md) — 实施状态
