# opencode-browser-tool

opencode 插件：Tauri 多 Webview 浏览器外壳 + 零注入视觉元素选取面板。对外只注册 **2 个工具**：`bt_cli`（命令行式分发器，内含 **52 条**浏览器自动化命令）与 `bt_set_media_mode`（摄像头/麦克风真伪切换），见下文「工具」。

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
sha256 校验（**幂等**：sha 与当前 release 期望值一致即跳过；**原子**替换；跨进程 `.lock`；Linux 下载后 `chmod 755`）。下载在插件初始化时即发起、后台进行、**不阻塞 opencode 启动**（`src/index.ts:859`）；未就绪时工具调用会给出
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

生成的配置还支持（可选项，见 `src/config/index.ts:55-77,254-288`）：

- `browsersPath` — 浏览器缓存目录（等价 `BT_BROWSERS_PATH`）
- `cacheDir` — 插件缓存根（默认 `~/.opencode/plugins-cache/opencode-browser-tool`，下设 `sessions/<profile>/<sid>`、`user-data/<profile>`、`exports`、`tmp`）
- `nodePath` — Node.js 可执行路径（等价 `BT_NODE_PATH`）
- `disabledTools` — **按已注册工具名**禁用工具。bt_cli 合并后注册的工具名只有 `bt_cli` 与 `bt_set_media_mode`（`src/index.ts:905-1004`、过滤逻辑 `src/index.ts:1034-1041`），因此只有写这两个名字才生效；旧的按命令拆分的工具名（`close`、`custom_user_agent`…）或不带前缀的命令名**不再有效、会被静默忽略**（自动生成配置的注释示例同样偏旧，以本节为准）。禁用 `bt_cli` 等于没有命令入口，慎用。
- `consoleBufferBytes` / `networkBufferBytes` — console / network 日志流的**字节预算**，默认各 **8 MiB**（8 388 608，跨 tab 全局共享）；超出按最旧逐出并计入 `dropped`（等价 `BT_CONSOLE_BUFFER_MB` / `BT_NETWORK_BUFFER_MB`，env 以 **MB** 计）
- `networkBurstWindowMs` — network 聚合窗口，默认 **2000 ms**：仅当与上一条同 key 且间隔 ≤ 该值才累加 `count`（等价 `BT_NETWORK_BURST_MS`）
- `consoleInject` — Linux console 注入猴补开关，默认 **Linux `true` / Windows `false`**；Windows 走 CDP 不需要注入（等价 `BT_CONSOLE_INJECT`）
- `webview2Args` — **仅 Windows** 的 WebView2 额外启动参数（透传为 `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS`，与进程已有值合并、已包含则不重复），默认四个参数（`src/config/index.ts:41-49`）：
  - `--disable-features=CalculateNativeWinOcclusion`（关窗口遮挡计算）
  - `--disable-background-timer-throttling` / `--disable-renderer-backgrounding` / `--disable-backgrounding-occluded-windows`（窗口被遮挡/后台时不节流、不挂起 renderer，否则 `ExecuteScript` 可能永不回调 → 同步等 UI 的调用永久阻塞）

  动机是 **RDP 断连/切后台时 DWM 合成暂停**会让依赖 GPU 合成的 WebView2 卡住 UI 线程。**默认保留 GPU 合成**（不加 `--disable-gpu-compositing`）；确实需要 CPU 合成排障时自行追加。写**空串**表示**不加任何参数**（`src/config/index.ts:386-396`）。等价 env `BT_WEBVIEW2_ARGS`（优先级更高）
- `downloadDir` — **下载保存目录模板**，默认 `<cache>/sessions/<profile>/<sid>/downloads`。占位符：`<cache>`（插件缓存根）、`<profile>`（配置名）、`<sid>`（会话 id）、`<sname>`（**会话标题**，已做文件名安全化：去掉 `\ / : * ? " < > |` 与控制字符、段尾 `.`/空格、截断 64 字符）。插件**展开后**以 `--download-dir` 传给 shell（详见「下载」一节）。等价 env `BT_DOWNLOAD_DIR`（env 优先）
- `downloadHistoryLimit` — 每个下载目录保留的**已完成**记录条数，默认 **64**（进行中不淘汰）。等价 env `BT_DOWNLOAD_HISTORY`

环境变量（优先级最高，`src/config/index.ts:306-331`）：
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
- `BT_CONSOLE_BUFFER_MB` / `BT_NETWORK_BUFFER_MB` — console / network 日志流字节预算（**MB**，默认各 8；对应配置项 `consoleBufferBytes` / `networkBufferBytes`）
- `BT_NETWORK_BURST_MS` — network 聚合窗口（ms，默认 2000；对应配置项 `networkBurstWindowMs`）
- `BT_CONSOLE_INJECT` — Linux console 注入开关（`1`/`0`；默认 Linux `1`、其它平台 `0`；对应配置项 `consoleInject`）
- `BT_WEBVIEW2_ARGS` — **仅 Windows**：WebView2 额外启动参数（覆盖配置项 `webview2Args`；**空串=不加任何参数**；默认见上方 `webview2Args` 的四个参数；与进程已有 `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS` 合并）
- `BT_DOWNLOAD_DIR` — 下载保存目录模板（覆盖配置项 `downloadDir`；占位符同配置项；插件展开后随 `--download-dir` 下发）
- `BT_DOWNLOAD_HISTORY` — 每个下载目录保留的已完成记录条数（默认 64；对应配置项 `downloadHistoryLimit`）

> **附着模式 vs 启动模式**：设置了 `BT_SHELL_URL`（优先）或 `BT_SHELL_PORT` 时进入附着模式——插件**不 spawn** 任何进程，直接连接该地址的外部 shell；两者都未设置时按启动模式 spawn 本地二进制（`BT_SHELL_PATH` 可覆盖查找路径）。附着模式下 `close` 命令只断开本地连接（**不关闭外部窗口、不结束外部进程**；进程不归插件管理）。
>
> **附着模式必须设置 `BT_SHELL_TOKEN`**：外部 shell 启动时的令牌必须与插件侧环境变量一致（见下方示例），否则插件启动即报 `client.attach_need_token`，不会去连一个注定 401 的端点。令牌未设置时 shell 会随机生成，因此手动启动的 shell 务必显式传 `--token`（或用同值环境变量）。
>
> **日志流配置只在启动模式生效**：`consoleBufferBytes` / `networkBufferBytes` / `networkBurstWindowMs` / `consoleInject` 由插件 spawn shell 时以 env 下发；**附着模式下插件只读不传**（外部进程不归插件管），要改缓冲/窗口/注入需在**启动外部 shell 的环境里**自行设置同名 `BT_*` 变量。`webview2Args` 同理：它是 spawn 时塞进子进程环境的 `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS`，附着模式下对已存在的外部进程无效（Windows 下需在启动外部 shell 的环境里设该变量）。

**本地 API 语义**：`/api/*` 只监听 `127.0.0.1`，且只服务插件进程——所有请求必须带 `x-bt-token`（服务端常量时间比较）。**`Origin` 规则（1.11.0 起）**：只有**面板自身 origin 白名单**（`http://tauri.localhost` / `https://tauri.localhost` / `tauri://localhost`，精确匹配、不带端口/子域）放行；**其它任何 `Origin`（`null`、外部站点、子域变体、带端口）仍一律 403**，插件 Node `fetch` 不带 `Origin` 不受影响。**CORS 只对面板 origin 最小恢复**：预检 `OPTIONS` 回 204 + 精确回显该 origin 的 `Access-Control-Allow-Origin`（允许 `POST, OPTIONS` 与 `content-type, x-bt-token` 头），实际响应（含 401）也带精确 ACAO + `Vary: Origin`，**永不通配 `*`**；非面板来源的响应不带任何 CORS 头、`OPTIONS` 仍 405。令牌在 shell 实例创建期确定、运行期不可变；若提供了 `--user-data-dir`，令牌还会写到 `<user-data-dir>/bt-auth-token`（Unix `chmod 600`）供排查。日志只打印令牌前 4 位与长度。**同进程的两个合法调用方各有例外**：面板 Webview 通过 Tauri `invoke("api_token")` 读取本实例令牌（只存 JS 内存，不写 DOM/HTML/日志）后再带 `x-bt-token` 调用，其 origin 也在白名单内（**令牌仍是最终闸门**）；页面注入脚本无法持有令牌，故服务端对**两个精确路径**豁免 `Origin` 与令牌校验（精确匹配、不做前缀匹配）：**`/api/page-event`** 与 **`/api/console-log`**（1.12.0 增）。两者的共同限制：**只写缓冲、不执行任何内容**（前者只更新标签 url/title/icon；后者只把页面 console 条目写进该标签的日志缓冲，body ≤ 256 KiB、超 200 条/秒丢弃并计数），**均无敏感回读**；`OPTIONS` 对它们同样不豁免（非面板 origin 仍 405）。残余风险：同机/页面侧可伪造这两个端点的写入（地址栏/标题错乱、日志缓冲被灌入伪造条目），但拿不到任何令牌或页面数据；若将来给它们加敏感字段或副作用，**必须重新评估豁免**。

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
            ├─ toolbar Webview（顶部：标签行 + 导航行 + 批注/截图/下载/面板/窗口按钮）
            ├─ page-1..page-N Webview（真多标签，每标签独立 webview，渲染目标网页）
            ├─ overlay Webview（透明覆盖层，批注高亮/截图选区/通知）
            ├─ panel Webview（右侧覆盖式浮层，AI 功能区 + 配置区）
            └─ newtab（内部新标签页，跟随主题）
```

- 浏览器渲染引擎（WebView2 / WebKitGTK / WKWebView，随平台）直接绘制到插件窗口
- 页面零注入残留脚本，弹框类元素可正常批注
- 所有工具调用通过 `POST http://127.0.0.1:PORT/api/{command}` 通信，并统一带鉴权头 `x-bt-token`（**除面板 origin 白名单外拒绝任何带 `Origin` 的请求**；面板 origin 走最小 CORS 预检，见上。鉴权豁免只有**两个精确路径**：页面事件桥 `/api/page-event` 与 console 回传 `/api/console-log`，均只写缓冲、无敏感回读）
- 除 spawn 本地 shell 外，也可用 `BT_SHELL_URL` / `BT_SHELL_PORT` **附着到已运行的 shell**（不 spawn，适用于 shell 跑在 WSL/Linux、插件跑在 Windows；**必须同时设置 `BT_SHELL_TOKEN`**，外部 shell 用同一令牌启动）
- 跨平台代码：Windows / Linux / macOS；**预编译二进制只发 Windows x64 与 Linux x64**，macOS 需自行构建（`cargo build --release`）且功能不完整：截图待实现（`rust/src/control/screenshot.rs:4`）、DevTools 无法真正关闭（`close_supported = cfg!(target_os = "linux")`，`rust/src/service.rs:1063`）、自定义 UA 未实现
- 真多标签：每标签独立 page Webview，切换不重载；`target=_blank` / `window.open` 链接通过 WebView2 原生事件拦截并打开为新标签。**关闭标签会释放该标签的页面实例**（预创建池的 3 个 webview 保留复用，池外动态创建的实例直接销毁），反复开关标签不会累积页面进程
- 地址栏/标题/图标**事件驱动同步（无轮询）**：导航用 `on_navigation`（请求即更新）+ `on_page_load`（完成后读真实 `location.href`/`title`）；站内 SPA（`pushState`/`replaceState`/`popstate`/`hashchange`）与标题/图标变化由注入脚本上报 `POST /api/page-event`。`TabState.url` 只反映**最后一次真实导航请求**，内部/错误 scheme（`chrome-error:`/`about:`/`tauri:`/`data:` 等）不覆盖地址栏
- 刷新语义：标签处于错误页（`errored`）时，工具栏刷新 / `reload` 命令**重新导航原 URL**（而非刷错误页）；否则走原生 reload
- 窗口无标题栏，自定义窗口按钮在标签行右侧；最小尺寸 360×432（逻辑像素；内容区最小 360×360，含 72px 工具栏）

## 工具

插件**只注册 2 个工具**（`src/index.ts:905-1004`）：

| 工具 | 参数 | 说明 |
|------|------|------|
| `bt_cli` | `args`(string，可选) | 唯一入口：命令行式分发器，一条命令字符串执行一项浏览器操作；`args` 省略或为 `help` / `--help` / `-h` 时返回完整帮助（命令名、用法、说明、示例） |
| `bt_set_media_mode` | `mode`(string)：`simulate` \| `real`，可选 | 切换摄像头/麦克风设备：`simulate`=模拟设备（默认，配合 `fake_audio` / `fake_video`）；`real`=真实设备（不可用时回落 simulate）。省略 `mode` 只查询当前模式。切真实设备属敏感操作，单独保留工具以维持 `ask` 权限 |

### bt_cli 调用语法

```
bt_cli({ args: "<command> [--flag value ...]" })
```

- **命令名不带 `bt_` 前缀**：写 `click`、`navigate`、`list_tabs`，**不是** `bt_click` / `bt_navigate` / `bt_list_tabs`（旧文档里的 `bt_*` 名称既不是工具名也不是命令名）
- **分词**：按 shell 风格切分（`string-argv` 0.3.2，`src/index.ts:3` 引入、`:918` 使用），含空格的值用引号包裹，如 `fill --selector #q --value "hello world"`；**只按引号配对切分、不处理反斜杠转义**——值里要出现引号就改用**另一种引号**包外层（如 `get_visible_text --selector "[title='a b']"`），写成 `--value "a\"b"` 会把 `\` 原样带进值里
- **flag 解析**：第一个 token 为命令名，其余交给 `node:util.parseArgs`（`src/index.ts:782-801`）；**未知 flag 直接报错并回显该命令用法**（输出可自愈，不必先调 help）
- **敏感值 `@file:`**：值写成 `@file:<路径>` 时由插件读取文件内容作为实际值，**内容不进模型上下文**。flag 值上的 `@file:` 只允许读插件临时目录（全局 tmp 或本会话 tmp）内的文件，其余路径一律拒绝；`lookup` 生成的密码临时文件即用于 `fill --value @file:...`，位于全局 tmp 的用完即删。**`evaluate` 的位置参数项**额外允许读**工作区目录**内的脚本文件（见下方「evaluate：多脚本顺序执行」）；**`type --text @file:`** 与 `evaluate` 同口径（工作区目录 + 插件 tmp，且**不删除**源文件，便于反复输入同一段文本）
- **全局 flag `--private-session`**（任何命令都可带，`src/index.ts:782-801,938-945`）：用**调用者自己的 sid** 建私有浏览器实例，不与根会话共用；并且对**同一子会话粘住**——首次用过之后，后续不带该 flag 也继续私有（根会话带它等价于自身）。典型场景：子代理需要**独立的浏览器环境做隔离测试**（自己的 cookie/登录态与窗口，不污染主会话）。根会话自己带它等价于自身（合法但无实际差别）。注意它紧跟某个 string flag 时会被当成该 flag 的值（`parseArgs` 行为），要写就写在命令名之后或命令末尾
- **会话归属由插件自动透传**（见「多会话」）：实例按会话隔离，但 **task 子代理会话默认归并到根会话**——沿 `Session.parentID` 回溯到最顶层，用**根 sid** 作路由键，于是主会话与所有子会话共用同一 bt-shell 实例、同一数据目录 `sessions/<profile>/<根 sid>/`，窗口标题取根会话标题。`args` 里**不需要**也**不支持**传会话 id

示例：

```text
bt_cli({ args: "help" })
bt_cli({ args: "navigate --url https://example.com" })
bt_cli({ args: "click --selector #submit" })
bt_cli({ args: "fill --selector #q --value \"hello world\"" })
bt_cli({ args: "screenshot --selector .chart" })
```

### `--timeout`（下表标 ⏱ 的 18 条命令支持）

- 单位**毫秒**，`--timeout 800` 可让卡住的操作约 0.8s 返回（快失败）；**未列出的命令不接受该 flag**
- 钳制区间 `[100, 60000]`：插件侧（`src/index.ts:111-112,120-130`）与 Rust 服务端（`rust/src/service.rs:96-97,104-116`）**同口径二次钳制**（直连 HTTP 也安全）；非正整数直接报错拒绝；被钳制时输出如实标注 `(clamped to …ms)`，服务端响应体带 `timeoutMs` / `timeoutClamped`
- **未传时走各命令自己的默认，并非统一 15s**：eval / CDP / 截图 / `wait_for_selector` 默认 **15s**（`rust/src/ui/mod.rs:35`、`rust/src/control/cdp.rs:27`、`rust/src/control/screenshot.rs:23`、`rust/src/service.rs:609`）；点击类（click / hover / drag / press_key 等）的稳定等待默认 **700ms**（`rust/src/control/mouse.rs:19,177-178`）；`fill` / `select` 的读回校验默认 **2500ms**（`rust/src/control/input.rs:343`）

### `evaluate`：多脚本顺序执行（插件侧，零 Rust 改动）

位置参数按**给定顺序**逐项执行，`@file:<路径>` 读文件内容作为脚本，其余参数当作内联 JS：

```text
bt_cli({ args: "evaluate @file:scripts/hook.js @file:scripts/collect.js \"console.log(1)\"" })
bt_cli({ args: "evaluate \"1+1\"" })                                  # 单个内联脚本
bt_cli({ args: "evaluate --script \"1+1\"" })                          # 单项旧写法（输出仍为裸结果 "2"）
bt_cli({ args: "evaluate @file:a.js @file:bad.js --continue-on-error" })
```

- **串行、按顺序**：前一项 `await` 结束后才发下一项，绝不并行
- **默认遇错即停**（`stop`）；`--continue-on-error` 继续执行后续项（`continued`）
- **`--script` 只能单独用**：与位置参数同时给会报错（避免执行顺序歧义）
- **文件读取安全边界**：只允许读**工作区目录**（插件 `setAppDirectory` 记录的 opencode `directory`）与**插件 tmp**（全局 tmp / 本会话 tmp）内的文件；其余路径**直接拒绝且不读取**。相对路径相对**工作区目录**解析（不是插件 cwd），并做 `isInsideDir` containment 校验。单文件 ≤ **256 KiB**、合计 ≤ **1 MiB**，超限报错
- **文件项在执行前统一读入**：任一项路径越界 / 读取失败 / 超限 → 整次调用**不执行任何脚本**（不留半截副作用）

输出（每条结果截断到 500 字符；失败信息经 `sanitizeForModel`，不含绝对路径）：

```text
evaluate: 3 item(s), 2 ok, 1 failed (stop)
[1] file scripts/hook.js  ok  3ms  → <result>
[2] text                  ok  0ms  → <result>
[3] file scripts/bad.js   FAILED  1ms  → SyntaxError: ...
```

全部成功时首行是 `evaluate: N item(s), N ok`；`--continue-on-error` 时标注 `(continued)`。单项 `--script` 保持旧输出（结果 JSON + 可能的 timeout 提示）。

### 持久注入规则（`inject`）

把 JS 存成**规则**：文档加载完成时若当前 URL 命中，就自动执行（不必每次 `evaluate`）。

```text
bt_cli({ args: "inject add --match \"*ygpm.zhihjy.cn*\" --file scripts/hook.js --order 10" })
bt_cli({ args: "inject add --match \"re:^https://example\\.com/\" --text \"console.log(1)\" --disabled" })
bt_cli({ args: "inject list" })
bt_cli({ args: "inject test --url https://ygpm.zhihjy.cn/a" })   # 只算命中，不执行
bt_cli({ args: "inject disable --id r1" })
```

- **存储**：`sessions/<profile>/<sid>/inject/rules.json`（随会话数据目录，**会话删除即清理**）；写入用临时文件 + rename **原子替换**
- **匹配语法**：`--match` 传 glob（`*` 通配，如 `*host*`）或 `re:<正则>`（如 `re:^https://x\.com/`）；多条命中按 `--order` **升序**执行（未给 `--order` 时由服务端取默认）
- **执行时机**：该标签文档加载完成（Windows `Page.loadEventFired`、Linux `WebKitWebView::load-changed` FINISHED）→ 取该标签当前 URL → 命中 enabled 规则 → 逐条**串行**执行 items（单条 15s 超时）；失败**不阻塞页面加载**、不向页面抛异常，结果只写回规则的 `last`
- **失败自动禁用**：`last` 记录 `ok` / `failed` / `error`；同一规则连续失败 **3** 次（`failStreak >= 3`）自动置 `enabled=false`，`last.error` 保留原因（`inject list` 显示错误首行）
- **上限**：规则 ≤ **20** 条；单条 items 合计 ≤ **256 KiB**；全部 ≤ **1 MiB**。超限在插件侧直接拒绝，不把坏规则写进存储
- **`--file` 读取**：只允许**工作区目录**（插件记录的 opencode `directory`）与**插件 tmp**（全局 tmp / 本会话 tmp），与 `evaluate` 同一套 containment；单条 ≤ **256 KiB**。文件内容在插件侧转成 `{ kind:"text", code, name:"<basename>" }` 发送，**Rust 不接触路径**；`--text` 的 name 固定为 `text`
- **`inject list` 只给结构**：`id / order / enabled / match / items / last`（ok、failed + 最近错误首行），明细只显示每个 item 的 `name` + 字节数，**绝不回显脚本内容**
- **与 `evaluate` 的区别**：`evaluate` 是**一次性**在当前页面执行并返回结果；`inject` 是**持久规则**，命中 URL 后每次加载都执行，结果只写回规则（不回模型），需用 `inject list` 查看

### 命令参考（52 条，与 `COMMANDS` 一致）

> 唯一真源：`src/index.ts:521-757` 的 `COMMANDS`（本表与之逐条对应，条数、flag 名、可选性均以代码为准）。说明取自 `src/i18n/en.ts` 的 `cli.cmd.*`（`src/i18n/en.ts:49-100`，中文对照见 `src/i18n/zh.ts`）。参数列中**不带 `[]` 者为必填**；⏱ = 支持 `--timeout ms`。完整命令/参数/help 以 `bt_cli({ args: "help" })` 的实际输出为准。

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
| `type` ⏱ | `--text <文本\|@file:<路径>>` 必填，`[--selector] [--enter]` | **向当前焦点插入文本（不清空已有内容）**；`--selector` 先聚焦该元素，`--enter` 插入后回车；`--text` 支持 `@file:`（与 `evaluate` 同一套 containment）。xterm/终端/contenteditable 用它，表单替换用 `fill` |
| `upload_file` ⏱ | `--selector --filePath` 必填 | 上传文件到输入框 |
| `screenshot` ⏱ | `[--selector]` | 截取页面/元素截图（返回图片附件；带 `--selector` 先滚动到元素再截） |
| `evaluate` ⏱ | `[<js 文本>\|@file:<路径>]...`（或单个 `--script`） | 在页面执行 JS，返回结果。位置参数按**顺序**逐项执行（`@file:` 读脚本文件，其余为内联 JS）；默认遇错即停，`--continue-on-error` 继续；`--script` 只能单独用（与位置参数同时给会报错） |
| `inject` | `[add --match <glob\|re:...> [--file <path>]... [--text <code>]... [--order N] [--disabled] \| list \| remove\|enable\|disable --id <id> \| clear \| test --url <url>]` | 持久注入规则：URL 命中时在文档加载完成后执行 items（新增/列表/删除/清空/启用/禁用/试算命中，详见「持久注入规则」一节） |
| `get_visible_text` | `[--selector]` | 获取页面可见文本 |
| `get_visible_html` | `[--selector --removeScripts --removeComments --maxLength]` | 获取页面 HTML 结构（`--removeScripts` / `--removeComments` 为布尔开关；`--maxLength` 超出时输出标注截断） |
| `console_logs` | `[--type --group burst\|none --phase --search --limit --tab --include-internal --clear --no-dedupe]` | 获取缓冲的控制台日志：`--type` = `all`/`log`/`info`/`warn`/`error`/`debug`/`exceptions`；`--group burst`（默认）合并同内容连续重复，`none` 逐条；`--phase` 过滤 `load`/`interactive`/`action:<cmd>`；`--tab N` 按标签过滤，N 是 `list_tabs` 打印的**0 基索引**（`--tab 0` = 第一个标签；不是内部 id）；`--include-internal` 显示浏览器内部日志（CDP `Log.entryAdded` 的 `source=violation\|deprecation`，如 `[DOM] Password field …`），默认不显示；`--limit` 默认 30；`--clear` 读取后清空；`--no-dedupe` 等价 `--group=none`。条目带 `(xN)`/`phase=`/`source=`（`cdp`/`injected`/`native`），异常附 stack 首行 |
| `network_logs` | `[--type --status 2xx\|3xx\|4xx\|5xx\|N --failed --search --group burst\|key\|none --phase --limit --tab --include-headers --reveal-secrets --clear --no-dedupe]` | 获取缓冲的网络请求：`--type` = `all`/`xhr`/`fetch`/`doc`/`script`/`image`/`font`/`other`；`--status` 支持区间写法或具体码；`--failed` 只看失败（`status=0` + `failure`）；`--group burst`（默认，同 key 且间隔 ≤ `networkBurstWindowMs` 才合并）/`key`（忽略 phase 与间隔）/`none`；`--tab N` 同 `console_logs`（`list_tabs` 的 0 基索引）；`--include-headers` 附加 headers（默认 `Authorization`/`Cookie`/`Set-Cookie` → `<redacted>`）；`--reveal-secrets` 显示原值（**必须配合 `--include-headers`**，单独使用会明确报错；**风险见下**）；`--clear` 读取后清空；`--no-dedupe` 等价 `--group=none` |
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
| `expect_response` | `--url` 必填 | 记录期望匹配的响应模式与起点（**不清空** `network_logs` 历史；`assert_response` 只匹配起点之后的请求） |
| `assert_response` | `--id` 必填 | 断言是否存在匹配的响应（返回 `url` + `status` 码） |
| `get_accessibility_tree` ⏱ | `[--selector --maxDepth]` | 获取可访问性树（role/name/value 嵌套）。`--maxDepth` 缺省 8；服务端硬上限 **depth 64 / 节点 5000**，超出静默截断并记日志（`rust/src/control/accessibility.rs:362,366,572`） |
| `list_records` | 无 | 列出面板批注/截图记录（索引/选择器/rect/说明；截图带 `+img`，已发送带 `[sent]`）。记录上限 **200** 条，超出丢最旧（`rust/src/ui/annotate.rs:461,467-475`） |
| `read_record_content` | `--id` 必填 | 按 id 读取单条记录详情（截图落盘为临时 PNG 并给出大小与路径） |
| `fake_audio` | `--kind` 必填，`[--data --freq --durMs --notes --digits --loop]` | 向 fake 麦克风注入音频（`kind`：tone/seq/dtmf/noise/audio/ambient/stop） |
| `fake_video` | `--kind` 必填，`[--data --url --loop]` | 向 fake 摄像头注入画面（`kind`：image/video/auto/stop） |
| `profile` | `[--set <name> \| --delete <name>]` | user-data 配置管理：空参列出；`--set` 切换默认配置（影响**后续新会话**的数据根，不重启运行中的浏览器）；`--delete` 删除（default/激活项除外） |
| `exports` | 无 | 显示导出目录与 Edge/Chrome 密码/书签导出指引 |
| `lookup` | `--csv <path> --url <site>` 必填 | 从密码 CSV 按站点匹配并返回 `@file:` 临时文件路径（密码不进上下文，用完即删） |
| `downloads` | `[--limit N --clear --cancel <id> --delete <id>]` | 列出本会话下载：**按目录分组**（组标题 `目录尾段 (n)`，行内 `文件名  已收/总量  状态[ 百分比\|不确定]  host`）；`--limit N` 限制每组条数；`--cancel <id>` 取消进行中（失败回传 shell 原因）；`--delete <id>` 删除单条记录**并删除磁盘文件**（进行中需先取消；文件已不存在时仍删记录并说明原因）；`--clear` 清空已完成/失败记录（进行中保留）。详见「下载」一节 |
| `download_dir` | `[--set <绝对路径>]` | 无参输出当前下载目录；`--set` 切换**本会话**目录（不存在自动创建；盘符根/系统目录被拒绝，错误原样回传） |

### `fill` vs `type` vs `press_key`（三种输入方式别混用）

| 命令 | 语义 | 典型场景 |
|---|---|---|
| `fill --selector --value` | **替换**：先聚焦 → 全选 → 插入 → 读回校验 | 普通表单输入框（`input`/`textarea`）：要把原值整段换成新值 |
| `type --text [--selector] [--enter]` | **插入**：不选中、不清空，直接插到光标处（可选先聚焦、可选回车） | xterm/终端、`contenteditable` 富文本、需要逐段追加或在已有内容后继续输入 |
| `press_key --key [--selector]` | **单个按键**（含 Enter/Tab/方向键/Delete/F1-F12） | 触发快捷键、提交、删一个字符、方向键选择下拉项 |

- **为什么要有 `type`**：`fill` 是替换语义 + 读回校验，在 xterm 里会把终端已有内容清掉、且读不到 `input.value`；`press_key` 一次只能一个键，多字符文本要自己拼接。`type` 走同一条可信注入通道（Windows `Input.insertText`、Linux GDK 逐字符），一次插入整段文本，**不需要用 `evaluate` 伪造 `InputEvent`/`KeyboardEvent`**（伪造事件 `isTrusted=false`，不少终端/富文本组件不认）。
- **组合用法**：先 `type --text "ls" --enter`（终端里执行命令）；富文本追加后如未触发组件自身的保存逻辑，再配合 `press_key --key Tab` 或在表单场景改用 `fill`。

### 日志流能力矩阵（1.12.0）

| 能力 | Windows（WebView2 + CDP） | Linux（WebKitGTK） |
|---|---|---|
| console 采集 | `Runtime.enable` + `Log.enable`（`consoleAPICalled` / `entryAdded` / `exceptionThrown`） | 页面注入（`console.*` + `onerror` + `unhandledrejection` + 1s 重挂检测 + 同源 iframe 汇总，`sendBeacon` 批量回传）；开关 `consoleInject` |
| console `source` | `cdp` / `native`（浏览器自身日志） | `injected` |
| network 采集 | `Network.enable`（`requestWillBeSent` / `responseReceived` / `loadingFailed` / `loadingFinished`） | WebKitGTK `resource-load-started` + `notify::response`（补 method/mime/耗时/failed） |
| `initiator` | `<type> <scriptUrl>:<line>`，无则 `other` | `subresource` / `document`（**无脚本行号**） |
| `phase` | 主帧 `Page.frameNavigated` / `Page.loadEventFired` + 动作窗口（子帧导航不置 `load`） | `load-changed` + 动作窗口 |
| 失败请求 | `status=0` + `failure`（如 `net::ERR_ABORTED`） | 同样 `status=0` + `failure`（字段来自 WebKitGTK 事件） |

- **动作归因（两平台）**：工具命令 `click` / `fill` / `select` / `press_key` / `navigate` 等执行期间开窗、结束后保留 **3s 宽限**（硬上限 60s；并发命令取最新，窗口不叠加），窗口内条目记 `phase=action:<cmd>`。
- **跨导航保留**：日志缓冲在 shell 侧、按标签归属，导航不清空；关闭标签释放该标签记录，关闭会话释放全部。
- **默认预算 8 MiB**：console / network 各自独立预算，且**跨 tab 全局共享**（逐出全局最旧，不是每 tab 各 8 MiB）；超限丢最旧并累加 `dropped`（输出末尾标注）。

> ⚠ **`--reveal-secrets` 风险**：默认对 `Authorization` / `Cookie` / `Set-Cookie` 做 `<redacted>` 脱敏；`--reveal-secrets` 会把这些头**原样返回给模型并进入对话上下文**（可能含会话令牌/密码）。仅在排障必要时临时使用，用完及时收尾；不要把输出粘到外部系统。

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
- **窄模式（窗口逻辑宽 < 480）**：面板改为**填满整个窗口**（覆盖式，x=0；页面 webview 保持全宽在下层，关闭面板即见完整页面）；工具栏的批注/截图/下载/面板四按钮**去文字只留图标**（媒体查询 `@media (max-width: 479px)`，与 Rust `ui::NARROW_MODE_MAX_W=480` 同步维护；下载角标不是文字标签，窄模式下仍显示）。手机类预设（360~412 宽）天然落入窄模式，避免按钮溢出

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
- **记录列表** — 面板「AI 功能」区：批注/截图记录卡片（截图带缩略图），显示所属标签 URL 与说明。**记录上限 200 条**，超出丢最旧（`rust/src/ui/annotate.rs:461,467-475`）
- **发送** — 面板底部「发送」按钮：所有记录（批注文字 + 截图图片）推送到 opencode 对话；发送后**面板列表清空**（与旧 Playwright 工具一致，仅列表为空），但**记录本身保留**并标记 `[sent]`（`list_records` 仍可读取，截图也可通过 `read_record_content` 取到）。投递为**推送式**：shell 发送后主动通知插件（`POST /notify/<sid>`，按会话归属），插件收到即拉取并推送到对话（无轮询、无 2s 延迟）。**待发送队列上限 500 条**，超出丢最旧（`rust/src/ui/mod.rs:1369,1464-1466`）
- 批注和截图**必须填写说明**才能保存/发送，否则弹出提示

面板「配置」区：外观主题（跟随系统/浅色/深色）、设备预设下拉（等价 `set_device`）、开发者工具按钮（等价 `devtools`）。

## 下载

下载**按会话隔离**：每个会话有独立保存目录，默认 `<cache>/sessions/<profile>/<sid>/downloads`（`<cache>` = 插件缓存根，见「配置」的 `cacheDir`）。

- **默认目录模板**：配置项 `downloadDir`，支持占位符 `<cache>` / `<profile>` / `<sid>` / `<sname>`；`<sname>` 取**会话标题**并做文件名安全化（去掉 `\ / : * ? " < > |` 与控制字符、清理段尾 `.`/空格、截断 64 字符）。默认值 `<cache>/sessions/<profile>/<sid>/downloads`
- **可配置项**：`downloadDir`（模板）、`downloadHistoryLimit`（每组保留的已完成记录条数，默认 **64**）；env `BT_DOWNLOAD_DIR` / `BT_DOWNLOAD_HISTORY` 优先级更高，两者都在插件启动时读取（改完需重启 opencode）
- **下发方式**：spawn 模式由插件**展开模板**后以 `--download-dir <展开后的路径>` 传给 shell，并按 `--user-data-dir` 从模板派生 `<profile>`/`<sid>`；为避免 shell 用另一套规则二次展开，插件会从子进程环境中**移除** `BT_DOWNLOAD_DIR`。**附着模式（`BT_SHELL_URL` / `BT_SHELL_PORT`）不传**：外部 shell 的下载目录由它的启动参数/环境决定，插件无法影响
- **持久化**：`download_dir --set <绝对路径>` 会把目录写进 `<会话数据目录>/download-dir.txt`，shell 启动时**优先读该文件**，其次才用 `--download-dir`（所以重启后目录保持）。允许任意绝对路径，但**拒绝**盘符根（`C:\`、`D:\`…）与系统目录（Windows `%SystemRoot%`/`%ProgramFiles%`/`%ProgramFiles(x86)%`/`%ProgramData%`；Linux `/`、`/etc`、`/usr`、`/bin`、`/sbin`、`/lib*`、`/boot`、`/dev`、`/proc`、`/sys`），其它路径自动 `mkdir -p`
- **分组与上限**：列表**按存储目录分组**，每组按开始时间倒序、默认保留最近 **64** 条已完成记录（`downloadHistoryLimit` 可调，**进行中不淘汰**）。目录切换后，旧目录的记录仍按原目录分组显示
- **面板视图切换**：右侧面板「AI 功能」页顶部有 `记录` / `下载` 两个视图按钮（窄窗口只留图标）；下载视图按目录分组、**每组默认收起**，点组标题展开。组内每行显示文件名、进度条（未知总量为不确定动画）、`已收/总量`、状态与来源 host，进行中可**取消**，组头可**打开所在文件夹**。面板可见时每 1s 轮询 `/api/downloads`
- **工具栏下载按钮 + 角标**：导航行 `批注/截图/下载/面板` 四按钮（窄窗口去文字只留图标，**角标始终可见**）。下载按钮右侧数字角标显示**进行中**数量（0 条时隐藏；超过 99 显示 `99+`）；新下载开始（`downloads-changed` 的 `progress`）时角标**脉冲 1s** 且按钮 `title` 变为 `下载：<文件名>`。计数来自 `/api/downloads`（节流 ≥500ms），**与面板是否打开无关**。点击按钮 → `invoke("panel_show_downloads")`：由 Rust 显示面板并广播 `downloads-view-requested`，面板据此切到「下载」视图并立即刷新（无下载时也切过去，显示「暂无下载」）。工具栏不做下拉（高度放不下）
- **删除记录 + 文件**：下载视图每行有「删除」按钮（trash 图标；窄窗口只留图标，`title` 给出说明），点击后删除该条记录**并删除磁盘文件**，成功提示 `已删除记录与文件`；文件已不存在/删不掉时仍会移除记录，并如实说明原因。进行中的行按钮禁用（先取消）。模型侧用 `downloads --delete <id>`
- **面板设置目录**：「配置」页「下载」分组可直接编辑当前目录，`保存` = 切换并持久化，`恢复默认` = 删除 `download-dir.txt` 并回到默认模板目录（响应里的 `defaultDir`），`打开文件夹` = 在系统文件管理器中打开当前目录；保存失败把 shell 的错误原文显示在该行（窄窗口按钮自动换行）
- **模型命令**：`downloads [--limit N]` / `downloads --cancel <id>` / `downloads --delete <id>` / `downloads --clear` / `download_dir [--set <绝对路径>]`，输出格式见「命令参考」
- **右键另存为**：页面里对图片/链接右键「另存为」也会保存到**当前下载目录**并记入下载列表（Windows；Linux 保持系统对话框）
- **整页另存**：页面里按 `Ctrl+S` 整页另存同样被接管（Windows）：系统对话框被压制，直接存到**当前下载目录**并记入列表，来源标记 `kind = "save-page"`；格式固定为**单文件 HTML**（不生成资源目录），图片等资源保持远程引用
- **记录持久化**：下载记录写入 `<会话数据目录>/download-history.json`（会话数据目录根下，**不放**下载目录，避免误删；形状同 `downloads` 输出的条目）；旧版 `<会话数据目录>/downloads/history.json` 在启动时一次性迁移并删除；变更后防抖保存（静默 500ms，进度持续刷新时最多 2s 写一次），退出前强制落盘，原子写（临时文件 + rename）；坏 JSON 按空列表启动。上次进程被强杀时仍为「进行中」的记录，重启后标为**失败**（原因 `interrupted by restart`）
- **UI 右键菜单**：面板 / 工具栏 / 覆盖层 / 新标签页已禁用浏览器默认右键菜单（不再出现「刷新」「另存为」等）；网页内容区右键保留

```text
bt_cli({ args: "downloads" })
bt_cli({ args: "downloads --limit 10" })
bt_cli({ args: "downloads --cancel 3" })
bt_cli({ args: "downloads --delete 2" })
bt_cli({ args: "download_dir" })
bt_cli({ args: "download_dir --set D:\tmp\dl" })
```

`downloads` 输出示例（组标题 = 目录尾段 + 条数；行内为 `文件名  已收/总量  状态[ 百分比|不确定]  host`，失败行末附原因）：

```text
dl (2)
  report.pdf  12MB/45MB  下载中 27%  files.example.com
  setup.exe  3MB  下载中 不确定  cdn.example.com
proj (1)
  data.csv  700KB/700KB  已完成  example.com
```

## 多会话

浏览器实例**按会话隔离**：独立进程 / 独立端口 / 独立用户数据目录，窗口、标签、UA、批注与 cookie/存储互不共享（`sessionIsolation` / `browserType` 配置项及 `--session-isolation` / `--browser` 参数自 1.9.0 已删除，会话隔离是唯一行为；旧配置文件里的这两项会被忽略）。

**但「会话」不等于「窗口」**（1.11.1 起）：**task 子代理会话默认归并到根会话**。插件沿 `Session.parentID` 回溯到最顶层，用**根 sid** 作浏览器路由键（`browserKey`），主会话与它的**所有**子会话共用同一 bt-shell 实例、同一数据目录 `sessions/<profile>/<根 sid>/`，窗口标题取**根会话**标题——子代理调 `bt_cli` 不会再各开一个窗口。

- **回溯限制**：单跳查询 `client.session.get` 超时 **1.5s**，最多回溯 **5 跳**；失败 / 超时 / 成环 / 超限都回退调用者自己的 raw sid 且**不缓存**（下次调用重试），只打 warn 日志——不会把子会话错认成根并长期钉住。
- **主子默认共用，私有需显式**：要让某个子会话用自己的实例（独立 cookie/登录态/窗口做隔离测试），给它加全局 flag `--private-session`；该标志对**同一子会话粘住**（首次之后不带也保持私有）。
- **删除会话**：删**子会话**只清该 sid 的关闭标记 / 附着记录，**不关根实例、不删根数据目录**（与根共用一份）；删**根会话 / 无父会话 / 私有会话**时才回收实例与数据目录。
- 例外：附着模式（`BT_SHELL_URL` / `BT_SHELL_PORT`）连接外部单例 shell，放弃会话隔离。

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
