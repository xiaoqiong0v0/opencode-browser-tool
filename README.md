# opencode-browser-tool

opencode 插件：Tauri 多 Webview 浏览器外壳 + 零注入视觉元素选取面板。对外只注册 **2 个工具**：`bt_cli`（命令行式分发器，内含 **47 条**浏览器自动化命令）与 `bt_set_media_mode`（摄像头/麦克风真伪切换），见下文「工具」。

> 架构演进：V1-V3 使用 Playwright 注入式面板（存在弹框类元素无法批注的问题）；V4 改为 Rust Shell + 原始调试协议；V5 迁移到 Tauri 2 多 Webview（跨平台）。

## 安装

```jsonc
// opencode.json
{
  "plugin": ["@xiaoqiong0v0/opencode-browser-tool"]
}
```

## 二进制获取（bt-shell）

插件依赖 Rust 外壳 `bt-shell`。默认在**插件加载时（opencode 启动）就后台自动下载**对应平台的 Release 资产到
`~/.opencode/plugins-data/opencode-browser-tool/<平台三元组>/bt-shell[.exe]`（Windows 即
`%USERPROFILE%\.opencode\plugins-data\opencode-browser-tool\<triple>\bt-shell.exe`），并**就地覆盖**同一路径
（不按插件版本建目录）；同目录另有 `bt-shell.version.json` 记录该可执行文件的来源
（`tag` / `pluginVersion` / `asset` / `sha256` / `size` / `downloadedAt` / `sourceUrl`，`src/binary.ts:69-79`）。下载用 Release 的 `SHA256SUMS` 做
sha256 校验（**幂等**：sha 与当前 release 期望值一致即跳过；**原子**替换；跨进程 `.lock`；Linux 下载后 `chmod 755`）。下载在插件初始化时即发起、后台进行、**不阻塞 opencode 启动**（`src/index.ts:342-343`）；未就绪时工具调用会给出
明确提示（下载进度 / 失败原因），不会假装成功。

平台支持与系统依赖：

- Windows x64 → `x86_64-pc-windows-msvc`（WebView2，系统自带）
- Linux x64 → `x86_64-unknown-linux-gnu`，需系统库 `libwebkit2gtk-4.1` / `libgtk-3`（及 appindicator / rsvg 等，见 [docs/linux-support.md](./docs/linux-support.md) §5）
- 其它平台：暂不提供预编译，请用 `BT_SHELL_PATH` 指定自行构建的二进制

> 代理：下载优先调用系统 `curl`（自动遵循 `HTTPS_PROXY`/`HTTP_PROXY`/`NO_PROXY`）；无 curl 时回退 Node `fetch`。
>
> **本地开发/调试**：插件优先使用本地 Rust 构建（按 opencode 传入的目录定位 `rust/target/release`，也支持 `CARGO_TARGET_DIR`、源码树的 `<install>/../rust/...` 与 `BT_SHELL_PATH`）；选中的路径与来源会写进日志 `Shell binary: <path> (source=local-dev|env|cargo-target-dir|download-cache)`；找不到时自动回落下载缓存（普通用户不受影响）。

## 版本与产物策略

- **中间位（minor）**：只要**动了 Rust / 要发新 exe**，就升 `x.Y.0`（如 `1.2.x → 1.3.0`），并为该 `x.Y.0` 新建/更新 Release 挂上二进制。
- **末位（patch）**：**只改插件**（TS/配置/文档，不改 Rust）时只升末位（如 `1.2.0 → 1.2.1`），**不新发二进制**。
- **二进制产物永远只挂在 `x.Y.0` 的 Release 上**：插件的 patch 版本**自动推导**出对应的二进制 tag（把 patch 归 0）：插件 `1.2.1` → 二进制 tag `v1.2.0`；`1.3.0` → `v1.3.0`；`1.3.0-beta.1` → `v1.3.0`。
- 因此纯插件改动升级时**复用同一份二进制**，不会重下；只有当推导出的 `x.Y.0` 与缓存中二进制的 sha 不一致（发新 exe）时才重新下载。
- 如需指向别的二进制发布（如临时验证），用 `BT_SHELL_VERSION` 显式覆盖——它表示**二进制发布版本**（形如 `1.2.0`），**原样使用、不做归 0 推导**。

## 配置

配置文件：`~/.config/opencode/browser-tool.jsonc`（首次运行自动生成，默认值均为 `en`）

```jsonc
{
  "panelLang": "en",
  "toolLang": "en"
}
```

项目级配置放 `<项目根目录>/.opencode/browser-tool.jsonc`，会覆盖全局。

生成的配置还支持（可选项，见 `src/config/index.ts:27-35,246-274`）：

- `browsersPath` — 浏览器缓存目录（等价 `BT_BROWSERS_PATH`）
- `cacheDir` — 插件缓存根（默认 `~/.opencode/plugins-cache/opencode-browser-tool`，下设 `sessions/<profile>/<sid>`、`user-data/<profile>`、`exports`、`tmp`）
- `nodePath` — Node.js 可执行路径（等价 `BT_NODE_PATH`）
- `disabledTools` — **按已注册工具名**禁用工具。bt_cli 合并后注册的工具名只有 `bt_cli` 与 `bt_set_media_mode`（`src/index.ts:376-463`、过滤逻辑 `src/index.ts:493-500`），因此只有写这两个名字才生效；旧的 47 个分工具名（`close`、`custom_user_agent`…）或不带前缀的命令名**不再有效、会被静默忽略**（自动生成配置的注释示例同样偏旧，以本节为准）。禁用 `bt_cli` 等于没有命令入口，慎用。

环境变量（优先级最高，`src/config/index.ts:223-240`）：
- `BT_LANG` — 同时设置 panelLang 和 toolLang
- `BT_PANEL_LANG` / `BT_TOOL_LANG` — 分别只设置面板 UI 语言 / 工具描述语言
- `BT_BROWSERS_PATH` — 浏览器缓存目录
- `BT_NODE_PATH` — Node.js 可执行路径
- `BT_SHELL_URL` — **附着模式**：连接到已运行的 shell（完整基地址，如 `http://127.0.0.1:18280`），优先级最高
- `BT_SHELL_PORT` — **附着模式**：仅指定端口，等价于 `http://127.0.0.1:<port>`
- `BT_SHELL_TOKEN` — **鉴权令牌**：本地 API 所有 `/api/*` 请求必须带 `x-bt-token`。**附着模式下必须设置**，且外部 shell 启动时要用同一值（`--token <token>` 或同值环境变量）；启动模式下由插件随机生成并通过 `--token` 传给 shell，无需手动设置
- `BT_SHELL_PATH` — **启动模式**：指定要 spawn 的 Rust shell 二进制路径（开发用；优先级最高，覆盖自动下载与本地构建）
- `BT_SHELL_NO_DOWNLOAD` — 设为 `1` 时不自动下载（状态 `disabled`；需自行放置二进制或配合 `BT_SHELL_PATH`）
- `BT_SHELL_DOWNLOAD_BASE` — 覆盖下载根（默认 GitHub Release `https://github.com/xiaoqiong0v0/opencode-browser-tool/releases/download`），可指向镜像/自建
- `BT_SHELL_VERSION` — 显式指定**二进制发布版本**（形如 `1.2.0`，即 `x.Y.0` 的 Release；**原样使用、不做归 0 推导**；不设时由插件版本自动推导）
- `BT_SHELL_NOTIFY_URL` — 插件通知地址（**推送式投递**；一般由插件自动通过 `--notify-url` / `/api/notify-url` 设置，仅在手动启动 shell 时需自行指定）

> **附着模式 vs 启动模式**：设置了 `BT_SHELL_URL`（优先）或 `BT_SHELL_PORT` 时进入附着模式——插件**不 spawn** 任何进程，直接连接该地址的外部 shell；两者都未设置时按启动模式 spawn 本地二进制（`BT_SHELL_PATH` 可覆盖查找路径）。附着模式下 `close` 命令只断开本地连接（**不关闭外部窗口、不结束外部进程**；进程不归插件管理）。
>
> **附着模式必须设置 `BT_SHELL_TOKEN`**：外部 shell 启动时的令牌必须与插件侧环境变量一致（见下方示例），否则插件启动即报 `client.attach_need_token`，不会去连一个注定 401 的端点。令牌未设置时 shell 会随机生成，因此手动启动的 shell 务必显式传 `--token`（或用同值环境变量）。

**本地 API 语义**：`/api/*` 只监听 `127.0.0.1`，且只服务插件进程——所有请求必须带 `x-bt-token`（服务端常量时间比较）；**任何带 `Origin` 头的请求一律 403**（浏览器跨源必带，插件 Node `fetch` 不带），服务端**不返回任何 CORS 头**、`OPTIONS` 直接 405。令牌在 shell 实例创建期确定、运行期不可变；若提供了 `--user-data-dir`，令牌还会写到 `<user-data-dir>/bt-auth-token`（Unix `chmod 600`）供排查。日志只打印令牌前 4 位与长度。**同进程的两个合法调用方各有例外**：面板 Webview 通过 Tauri `invoke("api_token")` 读取本实例令牌（只存 JS 内存，不写 DOM/HTML/日志）后再带 `x-bt-token` 调用；页面注入脚本无法持有令牌，故服务端仅对**精确路径 `/api/page-event`** 豁免 `Origin` 与令牌校验（不做前缀匹配），该端点只更新标签 url/title/icon、不执行任何操作。

**WSL / Linux 附着示例**（shell 跑在 WSL，插件跑在 Windows 宿主）：

```bash
# WSL 内：构建并启动 Linux 版 shell，监听 18280（令牌须与宿主 BT_SHELL_TOKEN 完全一致）
cd rust && cargo build --release
BT_SHELL_TOKEN=my-secret-token ./target/release/bt-shell --port 18280
# 等价：./target/release/bt-shell --port 18280 --token my-secret-token
```

```bash
# Windows 宿主：设置环境变量后重启 opencode（WSL2 支持从宿主经 localhost 访问）
set BT_SHELL_URL=http://127.0.0.1:18280
set BT_SHELL_TOKEN=my-secret-token
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
- 所有工具调用通过 `POST http://127.0.0.1:PORT/api/{command}` 通信，并统一带鉴权头 `x-bt-token`（无 CORS、拒绝任何带 `Origin` 的请求；唯一例外是页面事件桥 `/api/page-event`，见上）
- 除 spawn 本地 shell 外，也可用 `BT_SHELL_URL` / `BT_SHELL_PORT` **附着到已运行的 shell**（不 spawn，适用于 shell 跑在 WSL/Linux、插件跑在 Windows；**必须同时设置 `BT_SHELL_TOKEN`**，外部 shell 用同一令牌启动）
- 跨平台代码：Windows / Linux / macOS；**预编译二进制只发 Windows x64 与 Linux x64**，macOS 需自行构建（`cargo build --release`）且功能不完整：截图待实现（`rust/src/control/screenshot.rs:4`）、DevTools 无法真正关闭（`close_supported = cfg!(target_os = "linux")`，`rust/src/service.rs:1063`）、自定义 UA 未实现
- 真多标签：每标签独立 page Webview，切换不重载；`target=_blank` / `window.open` 链接通过 WebView2 原生事件拦截并打开为新标签。**关闭标签会释放该标签的页面实例**（预创建池的 3 个 webview 保留复用，池外动态创建的实例直接销毁），反复开关标签不会累积页面进程
- 地址栏/标题/图标**事件驱动同步（无轮询）**：导航用 `on_navigation`（请求即更新）+ `on_page_load`（完成后读真实 `location.href`/`title`）；站内 SPA（`pushState`/`replaceState`/`popstate`/`hashchange`）与标题/图标变化由注入脚本上报 `POST /api/page-event`。`TabState.url` 只反映**最后一次真实导航请求**，内部/错误 scheme（`chrome-error:`/`about:`/`tauri:`/`data:` 等）不覆盖地址栏
- 刷新语义：标签处于错误页（`errored`）时，工具栏刷新 / `reload` 命令**重新导航原 URL**（而非刷错误页）；否则走原生 reload
- 窗口无标题栏，自定义窗口按钮在标签行右侧；最小尺寸 360×432（逻辑像素；内容区最小 360×360，含 72px 工具栏）

## 工具

插件**只注册 2 个工具**（`src/index.ts:376-463`）：

| 工具 | 参数 | 说明 |
|------|------|------|
| `bt_cli` | `args`(string，可选) | 唯一入口：命令行式分发器，一条命令字符串执行一项浏览器操作；`args` 省略或为 `help` / `--help` / `-h` 时返回完整帮助（命令名、用法、说明、示例） |
| `bt_set_media_mode` | `mode`(string)：`simulate` \| `real`，可选 | 切换摄像头/麦克风设备：`simulate`=模拟设备（默认，配合 `fake_audio` / `fake_video`）；`real`=真实设备（不可用时回落 simulate）。省略 `mode` 只查询当前模式。切真实设备属敏感操作，单独保留工具以维持 `ask` 权限 |

### bt_cli 调用语法

```
bt_cli({ args: "<command> [--flag value ...]" })
```

- **命令名不带 `bt_` 前缀**：写 `click`、`navigate`、`list_tabs`，**不是** `bt_click` / `bt_navigate` / `bt_list_tabs`（旧文档里的 `bt_*` 名称既不是工具名也不是命令名）
- **分词**：按 shell 风格切分（`string-argv` 0.3.2，`src/index.ts:3` 引入、`:388-390` 使用），含空格的值用引号包裹，如 `fill --selector #q --value "hello world"`；**只按引号配对切分、不处理反斜杠转义**——值里要出现引号就改用**另一种引号**包外层（如 `get_visible_text --selector "[title='a b']"`），写成 `--value "a\"b"` 会把 `\` 原样带进值里
- **flag 解析**：第一个 token 为命令名，其余交给 `node:util.parseArgs`（`src/index.ts:285-291`）；**未知 flag 直接报错并回显该命令用法**（输出可自愈，不必先调 help）
- **敏感值 `@file:`**：值写成 `@file:<路径>` 时由插件读取文件内容作为实际值，**内容不进模型上下文**（`src/index.ts:404-435`）。只允许读插件临时目录（全局 tmp 或本会话 tmp）内的文件，其余路径一律拒绝；`lookup` 生成的密码临时文件即用于 `fill --value @file:...`，位于全局 tmp 的用完即删
- 会话归属由插件自动透传（每会话独立浏览器实例，见「多会话」），`args` 里**不需要**也**不支持**传会话 id

示例：

```text
bt_cli({ args: "help" })
bt_cli({ args: "navigate --url https://example.com" })
bt_cli({ args: "click --selector #submit" })
bt_cli({ args: "fill --selector #q --value \"hello world\"" })
bt_cli({ args: "screenshot --selector .chart" })
```

### `--timeout`（下表标 ⏱ 的 17 条命令支持）

- 单位**毫秒**，`--timeout 800` 可让卡住的操作约 0.8s 返回（快失败）；**未列出的命令不接受该 flag**
- 钳制区间 `[100, 60000]`：插件侧（`src/index.ts:80-81,96-98`）与 Rust 服务端（`rust/src/service.rs:96-97,104-116`）**同口径二次钳制**（直连 HTTP 也安全）；非正整数直接报错拒绝；被钳制时输出如实标注 `(clamped to …ms)`，服务端响应体带 `timeoutMs` / `timeoutClamped`
- **未传时走各命令自己的默认，并非统一 15s**：eval / CDP / 截图 / `wait_for_selector` 默认 **15s**（`rust/src/ui/mod.rs:35`、`rust/src/control/cdp.rs:27`、`rust/src/control/screenshot.rs:23`、`rust/src/service.rs:609`）；点击类（click / hover / drag / press_key 等）的稳定等待默认 **700ms**（`rust/src/control/mouse.rs:19,177-178`）；`fill` / `select` 的读回校验默认 **2500ms**（`rust/src/control/input.rs:343`）

### 命令参考（47 条）

> 唯一真源：`src/index.ts:101-268` 的 `COMMANDS`（本表与之逐条对应，条数、flag 名、可选性均以代码为准）。说明取自 `src/i18n/en.ts` 的 `cli.cmd.*`（`src/i18n/en.ts:24-70`，中文对照见 `src/i18n/zh.ts`）。参数列中**不带 `[]` 者为必填**；⏱ = 支持 `--timeout ms`。完整命令/参数/help 以 `bt_cli({ args: "help" })` 的实际输出为准。

| 命令 | 参数（usage 原文） | 说明 |
|------|------|------|
| `navigate` | `--url <地址>` 必填 | 导航浏览器到 URL（裸域名自动补 `http://`） |
| `click` ⏱ | `--selector` 必填 | 点击页面元素 |
| `fill` ⏱ | `--selector --value` 必填 | 填写输入框（`--value` 可传 `@file:` 路径） |
| `clear` ⏱ | `--selector` 必填 | 清空输入框 |
| `select` ⏱ | `--selector --value` 必填 | 选择下拉框选项 |
| `hover` ⏱ | `--selector` 必填 | 悬停到元素 |
| `drag` ⏱ | `--sourceSelector --targetSelector` 必填 | 拖拽元素到目标 |
| `press_key` ⏱ | `--key` 必填，`[--selector]` | 按下键盘按键；`--selector` 指定先聚焦的元素 |
| `upload_file` ⏱ | `--selector --filePath` 必填 | 上传文件到输入框 |
| `screenshot` ⏱ | `[--selector]` | 截取页面/元素截图（返回图片附件；带 `--selector` 先滚动到元素再截） |
| `evaluate` ⏱ | `--script` 必填 | 在页面执行 JS，返回结果 |
| `get_visible_text` | `[--selector]` | 获取页面可见文本 |
| `get_visible_html` | `[--selector --removeScripts --removeComments --maxLength]` | 获取页面 HTML 结构（`--removeScripts` / `--removeComments` 为布尔开关；`--maxLength` 超出时输出标注截断） |
| `console_logs` | `[--type --search --limit --clear]` | 获取浏览器控制台日志（`--clear` 读取后清空） |
| `go_back` | 无 | 浏览器后退，返回当前 URL |
| `go_forward` | 无 | 浏览器前进，返回当前 URL |
| `resize` | `--width --height` 必填 | 调整页面内容区尺寸（视口，非 OS 窗口；窗口高 = 内容高 + 工具栏 72px；下限 360×360，低于下限被钳制并标 `clamped`） |
| `set_device` | `[--name]` | 应用设备预设（不传 `--name` 列出预设；预设尺寸为内容区（视口）尺寸） |
| `devtools` | `[--action]` | 打开/关闭开发者工具：`open`/`close`/`toggle`（缺省 `toggle`）。Windows 无法真正关闭，`close` 如实返回不支持并提示用 F12 手动切换；Linux 可正常开关 |
| `reload` | 无 | 刷新当前页面；当前为错误页时**重新导航原 URL**（返回 `retried`） |
| `open_window` | 无 | 打开浏览器窗口（未启动则启动；已启动幂等） |
| `close` | 无 | 关闭浏览器（与手动关窗同一语义）。关闭后其它命令返回指引（`Browser is closed. Call open_window…`），**绝不自动重开**，需 `open_window` 显式打开；attach 模式只断开本地关联（不关外部窗口/进程） |
| `show_notification` | `--message` 必填，`[--type]` | 页面显示通知气泡（`--type`：`ok`/`bad`/`err`） |
| `scroll` | `[--direction --amount]` | 滚动页面（`--direction`：`up`/`down`/`left`/`right`，缺省 `down`；`--amount` 缺省 300px） |
| `wait_for_selector` ⏱ | `--selector` 必填 | 等待元素出现 |
| `click_and_switch_tab` | `--selector` 必填 | 点击元素；若触发新标签则自动切换，返回当前 URL |
| `iframe_click` ⏱ | `--iframeSelector --selector` 必填 | 在 iframe 内点击元素（仅同源可访问） |
| `iframe_fill` ⏱ | `--iframeSelector --selector --value` 必填 | 在 iframe 内填写输入框（仅同源可访问） |
| `get_browser_status` | 无 | **纯查询、不启动浏览器**：运行中返回 open/url/title/tabs 数量；未运行时返回未运行提示 + bt-shell 二进制状态（下载中/就绪/失败/禁用） |
| `list_tabs` | 无 | 列出所有打开的标签（索引+URL） |
| `switch_tab` | `--index` 必填 | 按索引（0 起）切换标签 |
| `new_tab` | `--url` 必填 | 打开新标签并导航（真多标签） |
| `close_tab` | `[--index]` | 关闭标签；省略则关当前激活标签。关闭会**释放该标签的页面实例**（池内 webview 回池复用，池外实例销毁） |
| `get_element_state` ⏱ | `--selector` 必填 | 元素存在/可见/文本/位置（rect） |
| `scroll_to_element` ⏱ | `--selector` 必填 | 滚动到元素可见；未到达/未稳定不吞错，输出带 `reached`/`stable`/`waitedMs` |
| `get_dropdown_options` ⏱ | `--selector` 必填 | 列出下拉框选项（当前选中项标 `*`） |
| `custom_user_agent` | `--userAgent` 必填 | 设置自定义 User-Agent（Windows/Linux 支持；macOS 未实现） |
| `expect_response` | `--url` 必填 | 记录期望匹配的响应模式，清空响应历史后重新捕获 |
| `assert_response` | `--id` 必填 | 断言是否存在匹配的响应（返回 `url` + `status` 码） |
| `get_accessibility_tree` ⏱ | `[--selector --maxDepth]` | 获取可访问性树（role/name/value 嵌套）。`--maxDepth` 缺省 8；服务端硬上限 **depth 64 / 节点 5000**，超出静默截断并记日志（`rust/src/control/accessibility.rs:355,359,551`） |
| `list_records` | 无 | 列出面板批注/截图记录（索引/选择器/rect/说明；截图带 `+img`，已发送带 `[sent]`）。记录上限 **200** 条，超出丢最旧（`rust/src/ui/annotate.rs:395,403-406`） |
| `read_record_content` | `--id` 必填 | 按 id 读取单条记录详情（截图落盘为临时 PNG 并给出大小与路径） |
| `fake_audio` | `--kind` 必填，`[--data --freq --durMs --notes --digits --loop]` | 向 fake 麦克风注入音频（`kind`：tone/seq/dtmf/noise/audio/ambient/stop） |
| `fake_video` | `--kind` 必填，`[--data --url --loop]` | 向 fake 摄像头注入画面（`kind`：image/video/auto/stop） |
| `profile` | `[--set <name> \| --delete <name>]` | user-data 配置管理：空参列出；`--set` 切换默认配置（影响**后续新会话**的数据根，不重启运行中的浏览器）；`--delete` 删除（default/激活项除外） |
| `exports` | 无 | 显示导出目录与 Edge/Chrome 密码/书签导出指引 |
| `lookup` | `--csv <path> --url <site>` 必填 | 从密码 CSV 按站点匹配并返回 `@file:` 临时文件路径（密码不进上下文，用完即删） |

### 元素截图行为（v1.7.0 起统一）

`screenshot --selector <css>` 固定流程：

1. **先滚动**：复用 `scroll_to_element`（立即 `scrollIntoView` 居中 + 轮询等滚动稳定）；未到达/未稳定**不吞错**，结果带 `reached`/`stable`/`waitedMs`，工具输出追加对应提示
2. **重测 rect**：滚动稳定后重新取元素视口矩形（覆盖懒加载/布局变化后的真位置）
3. **rect 完全落在视口内** → 常规 clip 路径（`captureBeyondViewport:false`，同旧版）
4. **rect 仍超出视口**（元素比视口还高/宽）→ **越界路径，整块截全**（不做尺寸上限）
   - Windows：CDP `Page.captureScreenshot` + `clip` + `captureBeyondViewport:true`
   - Linux：WebKitGTK `SnapshotRegion::FullDocument` 整页快照 + cairo 按文档坐标裁剪（该 API 只有 Visible/FullDocument 两档、无矩形参数）
5. 无 `selector` = 整视口截图（保持现状，无 `--full` 参数）

**坐标系（实测，v1.7.0）**：CDP `clip` **恒为文档坐标**（`captureBeyondViewport` true/false 均如此）——传视口坐标会在滚动后的页面截到错误区域（旧版元素截图在滚动页面上即踩此坑）；截图层内部统一做 视口坐标 + `scrollX/Y` 换算，调用方一律传视口坐标。

错误口径不变：`element not found: <sel>` / `element has zero size: <sel>`。**输出图片尺寸**：

- **Windows**：元素/区域截图走 CDP `clip`，`scale` 固定 `1`（`rust/src/control/screenshot.rs:148`）→ 输出为**选区 CSS 像素 1:1**，插件不额外乘 DPR；无 `--selector` 的整视口截图**不带 `clip`/`scale`**（`rust/src/control/screenshot.rs:150`），由 WebView2/CDP 默认整屏捕获、插件不做缩放
- **Linux**（v1.10.0 起）：WebKitGTK 快照面按**设备像素**输出，裁剪前用 webview 的 `scale_factor` 做 CSS→设备像素换算——`设备像素 = round(CSS 像素 × k)`，`k` 取 `WidgetExt::scale_factor`（非有限或 ≤0 时回退 `1.0`）（`rust/src/control/screenshot.rs:196-199,228-233,293-294,305-314`）。因此 **HiDPI（k>1）下输出尺寸 = 选区 CSS 尺寸 × k，不再是 1:1**；旧文档「CSS 像素 = 设备像素」的 1:1 假设（仅 WSLg dpr=1 时恰好成立）已作废。无 `--selector` 的 `Visible` 整视口快照同样按该缩放输出


**已知限制**

- **sticky/fixed 在越界路径只成像一次**，且两端位置不同：Windows 绘制在「截图瞬间它们在视口中的位置」（换算到文档坐标 ≈ scrollY 处）；Linux `FullDocument` 整页渲染下 `sticky` 停在自然流位置、`fixed` 绘制在**文档顶部**
- Linux（含 WSLg）截图路径已实测通过（见 `docs/implementation.md`）；**真实 Linux 桌面（GNOME/KDE）仍待复验**
- 折下懒加载图（`loading=lazy`）经 cache-bust 实测：滚动前 `complete=false`，自动滚动后加载完成、截图内容非空白

## 设备预设 / 尺寸语义

> ⚠️ **行为变更（v1.8.0）**：`resize` 与 `set_device` 的宽高一律指**页面内容区（视口）**尺寸，不再是 OS 窗口尺寸。窗口高 = 内容区高 + 工具栏 72px。**旧脚本若按窗口尺寸理解，高度会偏 72px**。API 返回同时给出 `content` 与 `window` 两组尺寸便于核对。

- **内容区下限 360×360**：`resize` 低于下限会被钳制，返回 `clamped:true`（工具输出也标注）；窗口 `min_inner_size` 同步为 360×432（= 360 内容高 + 72 工具栏）
- **窄模式（窗口逻辑宽 < 480）**：面板改为**填满整个窗口**（覆盖式，x=0；页面 webview 保持全宽在下层，关闭面板即见完整页面）；工具栏的批注/截图/面板三按钮**去文字只留图标**（媒体查询 `@media (max-width: 479px)`，与 Rust `ui::NARROW_MODE_MAX_W=480` 同步维护）。手机类预设（360~412 宽）天然落入窄模式，避免按钮溢出

内置 14 种设备预设（**内容区尺寸** + User-Agent），可通过 `set_device`（不带 `--name` 即列出清单）或右侧面板下拉框一键切换：

| 分类 | 预设（内容区尺寸） |
|------|------|
| 桌面 | `desktop-720p`（1024×720）, `desktop-1080p`（1920×1080）, `desktop-900p`（1440×900） |
| iPad | `ipad-pro-11`（834×1194）, `ipad-10`（820×1180）, `ipad-mini-6`（744×1133） |
| iPhone | `iphone-15-pro`（393×852）, `iphone-14`（390×844）, `iphone-13`（390×844）, `iphone-12`（390×844）, `iphone-se`（375×667） |
| Android | `pixel-7`（412×915）, `pixel-6`（411×914）, `galaxy-s23`（360×780） |

切换时自动调整窗口尺寸并运行时修改 UA（UA 修改 Windows/Linux 支持；macOS 未实现）。

## 批注 / 截图

工具栏导航行的 ✏️ 批注、📷 截图按钮进入对应模式（进入后自动收起面板）：

- **批注模式** — 悬停高亮元素（覆盖层自绘），点击元素弹出输入框填写说明；引擎层拦截，弹框/下拉类元素不失焦
- **截图模式** — 点击立即截全屏；也可进入截图模式后拖动框选区域（选区可拖拽手柄调整大小），保存时可按需裁剪（遮罩式选区，默认整图，双击还原）
- **记录列表** — 面板「AI 功能」区：批注/截图记录卡片（截图带缩略图），显示所属标签 URL 与说明。**记录上限 200 条**，超出丢最旧（`rust/src/ui/annotate.rs:395,403-406`）
- **发送** — 面板底部「发送」按钮：所有记录（批注文字 + 截图图片）推送到 opencode 对话；发送后**面板列表清空**（与旧 Playwright 工具一致，仅列表为空），但**记录本身保留**并标记 `[sent]`（`list_records` 仍可读取，截图也可通过 `read_record_content` 取到）。投递为**推送式**：shell 发送后主动通知插件（`POST /notify/<sid>`，按会话归属），插件收到即拉取并推送到对话（无轮询、无 2s 延迟）。**待发送队列上限 500 条**，超出丢最旧（`rust/src/ui/mod.rs:1369,1464-1466`）
- 批注和截图**必须填写说明**才能保存/发送，否则弹出提示

面板「配置」区：外观主题（跟随系统/浅色/深色）、设备预设下拉（等价 `set_device`）、开发者工具按钮（等价 `devtools`）。

## 多会话

自 1.9.0 起**每个 opencode 会话独享一个浏览器窗口**：独立进程 / 独立端口 / 独立用户数据目录（`sessions/<profile>/<sid>`），窗口、标签、UA、批注与 cookie/存储全部互不共享（`sessionIsolation` / `browserType` 配置项及 `--session-isolation` / `--browser` 参数自 1.9.0 已删除，会话隔离是唯一行为；旧配置文件里的这两项会被忽略）。例外：附着模式（`BT_SHELL_URL` / `BT_SHELL_PORT`）连接外部单例 shell，放弃会话隔离。

## 发布

版本规则见上文「版本与产物策略」：**动了 Rust / 发新 exe 才升中间位（minor）**，纯插件改动只升末位（patch）；**二进制资产只挂在 `x.Y.0` 的 Release 上**（插件 patch 版本由自身版本推导二进制 tag）。

```bash
# 1) 定版本（动 Rust 用 minor；纯插件用 patch）
npm version <x.y.z> --no-git-tag-version
# 2) 构建插件产物（npm run build = tsc，src/**/*.ts → dist/，增量、**不清空 dist**）
#    改过文件名/删过源文件时先手动清 dist 再建，避免孤儿 .js（Windows：Remove-Item -Recurse -Force dist）
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
- **先本地自测**：需覆盖安装到 opencode 插件缓存后重启验证；**本地覆盖安装的脚本需自备**（`.tmp/` 被 gitignore、仓库不含该文件），做法与脚本模板见技能 `xqv-plugin-release`。
- **部署顺序（exe 文件锁）**：若已有 opencode 实例在跑，`cargo build --release` 会在最后一步报 `failed to remove file ... (os error 5)`（产物不可信）→ 先**退出 opencode**（确认 shell 也退出）→ 构建 → 重新启动。TS 改动需 `npm run build` 重建 `dist/`。

## 开发

```bash
npm install
# 1) 前端产物（frontend/dist；tauri.conf.json 的 build.frontendDist 指向它，Rust 编译期嵌入）
npm run build:frontend        # 等价：cd frontend && npm install && npm run build
# 2) Rust 外壳（开发构建；发布用 cargo build --release）
cd rust && cargo build
# 3) 插件 TS → dist/
cd .. && npm run build
```

> **Windows 构建前置**：Tauri 的 Windows 资源需要 `rust/icons/icon.ico`（Tauri 默认图标路径）。该目录被 `.gitignore` 忽略、仓库不含图标源文件，需自行准备 `rust/icons/icon.ico`（可另放 `icon.png`）。Linux 构建不需要该图标。

## 日志

插件侧统一使用公共包 `@xiaoqiong0v0/opencode-plugin-logger`，项目内由 `src/logger.ts` 导出共用实例。日志写入
`~/.opencode/plugins-log/YYYYMMDD.log`，行格式 `[时间] [级别] browser-tool <内容>`；需在
`~/.config/opencode/plugin-logger.jsonc` 设 `"enabled": true`（默认关闭）。

> **插件侧禁止用 `console.*` 往 stdout/stderr 打日志**——那会污染 opencode TUI。`src/**` 内的 `console.*` 仅允许出现在独立 CLI 的 `isMain` 守卫内（`node dist/binary.js` 直接运行时）。
> Rust 侧（`bt-shell`）的 `println!`/`eprintln!` 默认会被 spawn 管道吞掉、不可见；插件在启动 shell 时把其 stdout/stderr **转发进 logger**，因此 Rust 日志同样能在上述文件中看到。

## 文档

- [docs/v5-tauri-architecture.md](./docs/v5-tauri-architecture.md) — V5 当前架构
- [docs/implementation.md](./docs/implementation.md) — 实施状态与关键经验
- [docs/linux-support.md](./docs/linux-support.md) — Linux 适配、硬性约定，及 §7「平台能力与降级策略」（可信通道/降级矩阵）
- [docs/linux-verification.md](./docs/linux-verification.md) — Kali/Linux 可信输入复验清单（含 `tests/manual/verify-linux.sh` 一键 15 项）
- [docs/README.md](./docs/README.md) — docs 索引
