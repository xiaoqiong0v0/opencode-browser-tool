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
- `BT_SHELL_URL` — **附着模式**：连接到已运行的 shell（完整基地址，如 `http://127.0.0.1:18280`），优先级最高
- `BT_SHELL_PORT` — **附着模式**：仅指定端口，等价于 `http://127.0.0.1:<port>`
- `BT_SHELL_PATH` — **启动模式**：指定要 spawn 的 Rust shell 二进制路径（开发用）

> **附着模式 vs 启动模式**：设置了 `BT_SHELL_URL`（优先）或 `BT_SHELL_PORT` 时进入附着模式——插件**不 spawn** 任何进程，直接连接该地址的外部 shell；两者都未设置时按启动模式 spawn 本地二进制（`BT_SHELL_PATH` 可覆盖查找路径）。附着模式下 `bt_close` 只关闭浏览器窗口并断开连接，**不会**结束外部 shell 进程（进程不归插件管理）。

**WSL / Linux 附着示例**（shell 跑在 WSL，插件跑在 Windows 宿主）：

```bash
# WSL 内：构建并启动 Linux 版 shell，监听 18280
cd rust && cargo build --release
./target/release/bt-shell --port 18280
```

```bash
# Windows 宿主：设置环境变量后重启 opencode（WSL2 支持从宿主经 localhost 访问）
set BT_SHELL_URL=http://127.0.0.1:18280
```

⚠ WSL 内启动 shell 时**不要**设置 `WEBKIT_DISABLE_DMABUF_RENDERER` / `WEBKIT_DISABLE_COMPOSITING_MODE`——它们会破坏透明合成（覆盖层渲染成黑块/透出桌面），详见 [docs/linux-support.md](./docs/linux-support.md)。

## 架构（V5）

```
opencode（Bun）
  └─ spawn("bt-shell", [--port ...]) → HTTP 127.0.0.1:PORT
       └─ Rust Shell（Tauri 2 单窗口 + 多 Webview）
            ├─ toolbar Webview（顶部：标签行 + 导航行 + 批注/截图/面板/窗口按钮）
            ├─ page-1..page-N Webview（真多标签，每标签独立 webview，渲染目标网页）
            ├─ overlay Webview（透明覆盖层，批注高亮/截图选区/通知）
            ├─ panel Webview（右侧覆盖式浮层，AI 功能区 + 配置区）
            └─ newtab（内部新标签页，跟随主题）
```

- 浏览器渲染引擎（WebView2 / WebKitGTK / WKWebView，随平台）直接绘制到插件窗口
- 页面零注入残留脚本，弹框类元素可正常批注
- 所有工具调用通过 `POST http://127.0.0.1:PORT/api/{command}` 通信
- 除 spawn 本地 shell 外，也可用 `BT_SHELL_URL` / `BT_SHELL_PORT` **附着到已运行的 shell**（不 spawn，适用于 shell 跑在 WSL/Linux、插件跑在 Windows）
- 跨平台：Windows / Linux / macOS
- 真多标签：每标签独立 page Webview，切换不重载；`target=_blank` / `window.open` 链接通过 WebView2 原生事件拦截并打开为新标签
- 窗口无标题栏，自定义窗口按钮在标签行右侧；最小尺寸 500×400（逻辑像素）

## 工具

| 类别 | 工具 |
|------|------|
| **导航** | `bt_navigate`, `bt_go_back`, `bt_go_forward`, `bt_reload`, `bt_close` |
| **交互** | `bt_click`, `bt_fill`, `bt_clear`, `bt_select`, `bt_hover`, `bt_drag`, `bt_press_key`, `bt_upload_file`, `bt_iframe_click`, `bt_iframe_fill` |
| **标签页** | `bt_list_tabs`, `bt_switch_tab`, `bt_new_tab`, `bt_close_tab`, `bt_click_and_switch_tab` |
| **信息** | `bt_screenshot`, `bt_evaluate`, `bt_get_visible_text`, `bt_get_visible_html`, `bt_console_logs`, `bt_get_browser_status`, `bt_get_element_state`, `bt_get_dropdown_options`, `bt_list_records`, `bt_expect_response`, `bt_assert_response`, `bt_get_accessibility_tree` |
| **滚动/等待** | `bt_scroll`, `bt_scroll_to_element`, `bt_wait_for_selector`, `bt_resize`, `bt_set_device` |
| **设备/工具** | `bt_custom_user_agent`, `bt_devtools`（开发者工具开关） |
| **其他** | `bt_save_as_pdf`, `bt_show_notification`, `bt_read_record_content` |

### 工具参数

| 工具 | 参数（类型） | 说明 |
|------|------|------|
| `bt_navigate` | `url`(string) 必填 | 导航到指定地址（裸域名自动补 http://） |
| `bt_click` | `selector`(string) 必填 | 点击 CSS 选择器匹配的元素 |
| `bt_fill` | `selector`, `value`(string) 必填 | 向输入框填入文本 |
| `bt_clear` | `selector`(string) 必填 | 清空输入框 |
| `bt_select` | `selector`, `value`(string) 必填 | 选择下拉框选项 |
| `bt_hover` | `selector`(string) 必填 | 悬停在元素上 |
| `bt_drag` | `sourceSelector`, `targetSelector`(string) 必填 | 从源元素拖到目标元素 |
| `bt_press_key` | `key`(string) 必填, `selector`(string) 可选 | 按键（如 Enter/Escape）；可指定聚焦元素 |
| `bt_upload_file` | `selector`, `filePath`(string) 必填 | 向文件输入框上传文件 |
| `bt_screenshot` | `selector`(string) 可选（当前忽略） | 截取当前视口，返回图片附件（PNG） |
| `bt_evaluate` | `script`(string) 必填 | 在页面执行 JS 并返回 JSON 结果 |
| `bt_get_visible_text` | `selector`(string) 可选 | 返回页面/元素可见文本 |
| `bt_get_visible_html` | `selector`, `removeScripts`(boolean), `removeComments`(boolean), `maxLength`(number) | 返回页面/元素 HTML |
| `bt_console_logs` | `type`(string), `search`(string), `limit`(number), `clear`(boolean) | 读取页面控制台日志 |
| `bt_go_back` | 无 | 浏览器后退，返回当前 URL |
| `bt_go_forward` | 无 | 浏览器前进，返回当前 URL |
| `bt_resize` | `width`, `height`(number) 必填 | 调整窗口尺寸（逻辑像素） |
| `bt_set_device` | `name`(string) 可选 | 应用设备预设（空参数列出 13 种预设） |
| `bt_devtools` | `action`(string)：`toggle`/`open`/`close` | 开关开发者工具 |
| `bt_reload` | 无 | 刷新当前页面，返回 URL |
| `bt_close` | 无 | 退出浏览器外壳 |
| `bt_show_notification` | `message`(string), `type`(string)：`ok`/`bad`/`err` | 显示页面通知气泡 |
| `bt_scroll` | `direction`(string)：`up`/`down`/`left`/`right`, `amount`(number) | 滚动页面 |
| `bt_wait_for_selector` | `selector`, `timeout`(number, ms) | 等待元素出现 |
| `bt_click_and_switch_tab` | `selector`(string) 必填 | 点击元素；若触发新标签则自动切换，返回当前 URL |
| `bt_iframe_click` | `iframeSelector`, `selector`(string) 必填 | 在 iframe 中点击（仅同源可访问） |
| `bt_iframe_fill` | `iframeSelector`, `selector`, `value`(string) 必填 | 在 iframe 中输入（仅同源可访问） |
| `bt_save_as_pdf` | 无 | ⚠ 当前架构不支持，调用明确报错 |
| `bt_get_browser_status` | 无 | 返回 open/url/title/tabs 数量 |
| `bt_list_tabs` | 无 | 列出标签页（索引+URL） |
| `bt_switch_tab` | `index`(number) 必填 | 按位置（0 起）切换标签 |
| `bt_new_tab` | `url`(string) 必填 | 新建独立标签页（真多标签）并导航 |
| `bt_close_tab` | `index`(number) 可选 | 关闭指定标签；缺省关闭当前激活标签 |
| `bt_get_element_state` | `selector`(string) 必填 | 返回元素可见性/文本/位置 |
| `bt_scroll_to_element` | `selector`(string) 必填 | 滚动到元素 |
| `bt_get_dropdown_options` | `selector`(string) 必填 | 列出下拉框选项 |
| `bt_custom_user_agent` | `userAgent`(string) 必填 | 设置自定义 User-Agent（仅 Windows 支持） |
| `bt_expect_response` | `url`(string) 必填 | 记录期望匹配的响应模式，清空响应历史后重新捕获 |
| `bt_assert_response` | `id`(string) 必填 | 断言是否存在匹配的响应（返回 `url` + `status` 码） |
| `bt_get_accessibility_tree` | `selector`, `maxDepth`(number) | 返回页面可访问性树（role/name/value 嵌套结构） |
| `bt_list_records` | 无 | 列出批注/截图记录（索引/标签/说明） |
| `bt_read_record_content` | `id`(number) 必填 | 读取单条记录详情 |

> 架构限制：`bt_save_as_pdf`（PDF 导出）在 Tauri WebView 架构下无法实现，调用会明确报错而非假成功。

## 设备预设

内置 13 种设备预设（窗口尺寸 + User-Agent），可通过 `bt_set_device` 或右侧面板下拉框一键切换：

| 分类 | 预设 |
|------|------|
| 桌面 | `desktop-1080p`, `desktop-1440p` |
| iPad | `ipad-pro-11`, `ipad-10`, `ipad-mini-6` |
| iPhone | `iphone-15-pro`, `iphone-14`, `iphone-13`, `iphone-12`, `iphone-se` |
| Android | `pixel-7`, `pixel-6`, `galaxy-s23` |

切换时自动调整窗口尺寸并运行时修改 UA（UA 修改仅 Windows 支持）。

## 批注 / 截图

工具栏导航行的 ✏️ 批注、📷 截图按钮进入对应模式（进入后自动收起面板）：

- **批注模式** — 悬停高亮元素（覆盖层自绘），点击元素弹出输入框填写说明；引擎层拦截，弹框/下拉类元素不失焦
- **截图模式** — 点击立即截全屏；也可进入截图模式后拖动框选区域（选区可拖拽手柄调整大小），保存时可按需裁剪（遮罩式选区，默认整图，双击还原）
- **记录列表** — 面板「AI 功能」区：批注/截图记录卡片（截图带缩略图），显示所属标签 URL 与说明
- **发送** — 面板底部「发送」按钮：所有记录（批注文字 + 截图图片）推送到 opencode 对话，发送后清空列表
- 批注和截图**必须填写说明**才能保存/发送，否则弹出提示

面板「配置」区：外观主题（跟随系统/浅色/深色）、设备预设下拉（等价 `bt_set_device`）、开发者工具按钮（等价 `bt_devtools`）。

## 多会话

父子会话共享一个浏览器。开启 `sessionIsolation: true` 后，每个 opencode 会话有独立上下文（cookies/存储隔离）。

## 发布

```bash
cd rust && cargo build --release
npm publish
```

只发布 `dist/`（`files: ["dist"]`）。

## 开发

```bash
npm install
cd rust && cargo build
npm run build
```

## 文档

- [docs/v5-tauri-architecture.md](./docs/v5-tauri-architecture.md) — V5 当前架构
- [docs/implementation.md](./docs/implementation.md) — 实施状态
