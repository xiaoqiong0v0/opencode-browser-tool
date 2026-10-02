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

## 二进制获取（bt-shell）

插件依赖 Rust 外壳 `bt-shell`。默认**首次使用时后台自动下载**对应平台的 Release 资产到
`~/.opencode/plugins-data/opencode-browser-tool/<平台三元组>/bt-shell[.exe]`（Windows 即
`%USERPROFILE%\.opencode\plugins-data\opencode-browser-tool\<triple>\bt-shell.exe`），并**就地覆盖**同一路径
（不按插件版本建目录）；同目录另有 `bt-shell.version.json` 记录该可执行文件的来源
（`tag` / `asset` / `sha256` / `size` / `downloadedAt` / `sourceUrl`）。下载用 Release 的 `SHA256SUMS` 做
sha256 校验（**幂等**：sha 与当前 release 期望值一致即跳过；**原子**替换；跨进程 `.lock`；Linux 下载后 `chmod 755`）。下载后台进行、**不阻塞 opencode 启动**；未就绪时工具调用会给出
明确提示（下载进度 / 失败原因），不会假装成功。

平台支持与系统依赖：

- Windows x64 → `x86_64-pc-windows-msvc`（WebView2，系统自带）
- Linux x64 → `x86_64-unknown-linux-gnu`，需系统库 `libwebkit2gtk-4.1` / `libgtk-3`（及 appindicator / rsvg 等，见 [docs/linux-support.md](./docs/linux-support.md) §5）
- 其它平台：暂不提供预编译，请用 `BT_SHELL_PATH` 指定自行构建的二进制

> 代理：下载优先调用系统 `curl`（自动遵循 `HTTPS_PROXY`/`HTTP_PROXY`/`NO_PROXY`）；无 curl 时回退 Node `fetch`。

## 版本与产物策略

- **中间位（minor）**：只要**动了 Rust / 要发新 exe**，就升 `x.Y.0`（如 `1.2.x → 1.3.0`），并为该 `x.Y.0` 新建/更新 Release 挂上二进制。
- **末位（patch）**：**只改插件**（TS/配置/文档，不改 Rust）时只升末位（如 `1.2.0 → 1.2.1`），**不新发二进制**。
- **二进制产物永远只挂在 `x.Y.0` 的 Release 上**：插件的 patch 版本**自动推导**出对应的二进制 tag（把 patch 归 0）：插件 `1.2.1` → 二进制 tag `v1.2.0`；`1.3.0` → `v1.3.0`；`1.3.0-beta.1` → `v1.3.0`。
- 因此纯插件改动升级时**复用同一份二进制**，不会重下；只有当推导出的 `x.Y.0` 与缓存中二进制的 sha 不一致（发新 exe）时才重新下载。
- 如需指向别的二进制发布（如临时验证），用 `BT_SHELL_VERSION` 显式覆盖——它表示**二进制发布版本**（形如 `1.2.0`），**原样使用、不做归 0 推导**。

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
- `BT_SHELL_PATH` — **启动模式**：指定要 spawn 的 Rust shell 二进制路径（开发用；优先级最高，覆盖自动下载与本地构建）
- `BT_SHELL_NO_DOWNLOAD` — 设为 `1` 时不自动下载（状态 `disabled`；需自行放置二进制或配合 `BT_SHELL_PATH`）
- `BT_SHELL_DOWNLOAD_BASE` — 覆盖下载根（默认 GitHub Release `https://github.com/xiaoqiong0v0/opencode-browser-tool/releases/download`），可指向镜像/自建
- `BT_SHELL_VERSION` — 显式指定**二进制发布版本**（形如 `1.2.0`，即 `x.Y.0` 的 Release；**原样使用、不做归 0 推导**；不设时由插件版本自动推导）

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
| **导航** | `bt_navigate`, `bt_go_back`, `bt_go_forward`, `bt_reload`, `bt_open_window`, `bt_close` |
| **交互** | `bt_click`, `bt_fill`, `bt_clear`, `bt_select`, `bt_hover`, `bt_drag`, `bt_press_key`, `bt_upload_file`, `bt_iframe_click`, `bt_iframe_fill` |
| **标签页** | `bt_list_tabs`, `bt_switch_tab`, `bt_new_tab`, `bt_close_tab`, `bt_click_and_switch_tab` |
| **信息** | `bt_screenshot`, `bt_evaluate`, `bt_get_visible_text`, `bt_get_visible_html`, `bt_console_logs`, `bt_get_browser_status`, `bt_get_element_state`, `bt_get_dropdown_options`, `bt_list_records`, `bt_expect_response`, `bt_assert_response`, `bt_get_accessibility_tree` |
| **滚动/等待** | `bt_scroll`, `bt_scroll_to_element`, `bt_wait_for_selector`, `bt_resize`, `bt_set_device` |
| **设备/工具** | `bt_custom_user_agent`, `bt_devtools`（开发者工具开关） |
| **媒体/本地** | `bt_fake_audio`, `bt_fake_video`, `bt_profile`, `bt_exports`, `bt_lookup` |
| **其他** | `bt_show_notification`, `bt_read_record_content` |

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
| `bt_screenshot` | `selector`(string) 可选 | 截取视口；指定 `selector` 时截该元素区域（PNG 图片附件） |
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
| `bt_get_browser_status` | 无 | 返回 open/url/title/tabs 数量 |
| `bt_list_tabs` | 无 | 列出标签页（索引+URL） |
| `bt_switch_tab` | `index`(number) 必填 | 按位置（0 起）切换标签 |
| `bt_new_tab` | `url`(string) 必填 | 新建独立标签页（真多标签）并导航 |
| `bt_close_tab` | `index`(number) 可选 | 关闭指定标签；缺省关闭当前激活标签 |
| `bt_get_element_state` | `selector`(string) 必填 | 返回元素可见性/文本/位置 |
| `bt_scroll_to_element` | `selector`(string) 必填 | 滚动到元素 |
| `bt_get_dropdown_options` | `selector`(string) 必填 | 列出下拉框选项 |
| `bt_custom_user_agent` | `userAgent`(string) 必填 | 设置自定义 User-Agent（Windows/Linux 支持；macOS 未实现） |
| `bt_open_window` | 无 | 打开浏览器窗口（未启动则启动；已启动幂等） |
| `bt_fake_audio` | `kind`(string) 必填，`data`/`freq`/`durMs`/`notes`/`digits`(string)，`loop`(boolean) | 向页面注入模拟麦克风音频 |
| `bt_fake_video` | `kind`(string) 必填，`data`/`url`(string)，`loop`(boolean) | 向页面注入模拟摄像头画面 |
| `bt_profile` | `set` / `delete`(string) 可选 | user-data 配置管理：空参列出；`--set` 切换并重启服务；`--delete` 删除（默认/激活项除外） |
| `bt_exports` | 无 | 显示导出目录与 Edge/Chrome 导出指引 |
| `bt_lookup` | `csv`, `url`(string) 必填 | 从密码 CSV 按站点匹配并返回凭据（写入临时文件，不进上下文） |
| `bt_expect_response` | `url`(string) 必填 | 记录期望匹配的响应模式，清空响应历史后重新捕获 |
| `bt_assert_response` | `id`(string) 必填 | 断言是否存在匹配的响应（返回 `url` + `status` 码） |
| `bt_get_accessibility_tree` | `selector`, `maxDepth`(number) | 返回页面可访问性树（role/name/value 嵌套结构） |
| `bt_list_records` | 无 | 列出批注/截图记录（索引/标签/说明） |
| `bt_read_record_content` | `id`(number) 必填 | 读取单条记录详情 |

## 设备预设

内置 13 种设备预设（窗口尺寸 + User-Agent），可通过 `bt_set_device` 或右侧面板下拉框一键切换：

| 分类 | 预设 |
|------|------|
| 桌面 | `desktop-1080p`, `desktop-1440p` |
| iPad | `ipad-pro-11`, `ipad-10`, `ipad-mini-6` |
| iPhone | `iphone-15-pro`, `iphone-14`, `iphone-13`, `iphone-12`, `iphone-se` |
| Android | `pixel-7`, `pixel-6`, `galaxy-s23` |

切换时自动调整窗口尺寸并运行时修改 UA（UA 修改 Windows/Linux 支持；macOS 未实现）。

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

版本规则见上文「版本与产物策略」：**动了 Rust / 发新 exe 才升中间位（minor）**，纯插件改动只升末位（patch）；**二进制资产只挂在 `x.Y.0` 的 Release 上**（插件 patch 版本由自身版本推导二进制 tag）。

```bash
# 1) 定版本（动 Rust 用 minor；纯插件用 patch）
npm version <x.y.z> --no-git-tag-version
# 2) 构建插件产物（清空 dist 再建，避免孤儿 .js）
npm run build
# 3) 提交 + 打 tag + 建 Release（tag = v<x.y.z>）
#    只有动 Rust 时才需要为 x.Y.0 上传各平台二进制 + SHA256SUMS：
gh release create v<x.y.z> --title v<x.y.z> --notes "..."
gh release upload v<x.y.z> <各平台二进制> SHA256SUMS --clobber
# 4) 发布 npm 包（只含 dist；不含二进制）
npm publish
```

- `SHA256SUMS` 每行 `<sha256>  <文件名>`（两空格），文件名不带路径。
- npm 包只带 `dist/`（`files: ["dist"]`）；原生二进制由运行时从 Release 下载（见「二进制获取」）。
- **先本地自测**：可用 `.tmp/publish-local.ps1` 覆盖到 opencode 插件缓存后重启验证（做法与脚本模板见技能 `xqv-plugin-release`）。
- **部署顺序（exe 文件锁）**：若已有 opencode 实例在跑，`cargo build --release` 会在最后一步报 `failed to remove file ... (os error 5)`（产物不可信）→ 先**退出 opencode**（确认 shell 也退出）→ 构建 → 重新启动。TS 改动需 `npm run build` 重建 `dist/`。

## 开发

```bash
npm install
cd rust && cargo build
npm run build
```

## 文档

- [docs/v5-tauri-architecture.md](./docs/v5-tauri-architecture.md) — V5 当前架构
- [docs/implementation.md](./docs/implementation.md) — 实施状态与关键经验
- [docs/linux-support.md](./docs/linux-support.md) — Linux 适配、硬性约定，及 §7「平台能力与降级策略」（可信通道/降级矩阵）
- [docs/linux-verification.md](./docs/linux-verification.md) — Kali/Linux 可信输入复验清单（含 `tests/manual/verify-linux.sh` 一键 15 项）
- [docs/README.md](./docs/README.md) — docs 索引
