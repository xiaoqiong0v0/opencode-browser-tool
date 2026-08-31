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
- 窗口最小尺寸 500×400（逻辑像素），防止拖到过小导致布局异常；窗口内为顶部工具栏（标签+地址栏）+ 页面区 + 可弹出的右侧覆盖式面板

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

## 批注面板

- **批注模式** — 悬停高亮元素（覆盖层自绘），点击添加批注；引擎层拦截，弹框/下拉类元素不失焦
- **截图工具** — 框选区域，添加说明
- **发送全部** → 数据推送到 opencode 对话
- **设备预设下拉** — 快捷切换 13 种设备（等价 `bt_set_device`）
- **开发者工具按钮** — 打开/关闭 WebView2 开发者工具（等价 `bt_devtools`）

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
