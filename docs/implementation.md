# 实施状态

## 当前架构（V5：跨平台 Tauri 2 多 Webview）

单窗口 + 多 Webview（toolbar / page-1..N 真多标签 / overlay / panel / newtab），前端 TS 工程化，详见 [v5-tauri-architecture.md](./v5-tauri-architecture.md)。

### 已完成
- [x] V5.1 Tauri 骨架：单窗口 + 三 Webview（页面/覆盖层/面板），页面桥注入
- [x] V5.2 控制协议：eval 统一封装（导航/点击/填写/文本/状态/滚动/等待/查询）
- [x] V5.3 覆盖层批注：透明 Webview + Canvas，悬停高亮/点击标记/右键退出
- [x] V5.4 面板：TS 前端（列表/按钮/说明输入），Rust 命令接通，发送队列+插件端轮询推送
- [x] V5.5 HTTP 服务对接 + 插件集成，全链路端点回归通过
- [x] V5.6 截图 Windows CDP 实现（PNG base64 验证通过，支持 clip 区域截图）
- [x] V5.7 窗口 UI：无标题栏 + 自定义窗口按钮、主题系统、真多标签（每标签独立 webview）、newtab、事件驱动状态同步
- [x] V5.8 批注/截图双模式：遮罩式选区裁剪、保存需说明、发送后清空、记录卡片化、面板 toast
- [x] V5.9 真多标签接 HTTP + 新窗口拦截（target=_blank → 新标签）+ z-order 置顶
- [x] V5.10 工具补齐：expect/assert 网络响应捕获、可访问性树、插件取字段修复
- [x] 插件命令与 Rust 端点已全部对齐；**跨平台可用性**：Windows 全量可信，Linux 除 `upload_file`/`drag` **明确报错**外均可用（无 CDP/GDK 可信通道时的降级与标注见 `linux-support.md` §7），macOS 未实现
- [x] 依赖清理：移除 adm-zip/esbuild/tsx 旧 Playwright 残留与 test/ 目录；lucide 移 devDependencies
- [x] 截图 Linux 分支：WebKitGTK `WebView::snapshot`（Visible 区域）+ cairo 裁剪 → PNG base64
- [x] 可信输入全链路：Windows CDP（`Input.dispatchKeyEvent`/`dispatchMouseEvent`/`DOM.setFileInputFiles`/鼠标序列拖拽）、Linux GDK `gdk_event_put`；统一降级策略（可信优先，无通道则 DOM/JS 兜底并在返回标注 `degraded`，原本不可用者明确报错）
- [x] 时序/就绪：`navigate`/`go_back`/`go_forward` 等"确实发生导航（URL 或 `performance.timeOrigin` 变化）且新文档就绪"，`scroll` 等位置稳定，均**有界超时并如实报告**；`scroll_to_element` 支持内层滚动容器
- [x] 二进制分发：插件启动后台从 GitHub Release 下载到固定路径并校验/原子替换/并发锁；版本策略（二进制只挂 `x.Y.0`、插件自动推导 tag）
- [x] 纯查询命令不懒启动：`get_browser_status` 走 `probeStatus()`（`client.ts:667`，短超时 HTTP 探测，**不 spawn**）；未运行时返回未运行 + bt-shell 二进制状态
- [x] 发送（批注/截图）修复（以旧 `opencode-playwright-tool` 为参考）：面板"发送"**只发文本**（与参考一致；`session.prompt` 的 parts 不支持图片 —— `image/*` 走 `image.normalize` 返回 400，参考实现从不这么发）；截图图片走**工具结果附件**交付：`attachments:[{type:"file", mime:"image/png", url:"data:image/png;base64,…"}]`（关键：opencode 只接受 `url` 以 `data:` 开头且含 `,` 的附件，旧形状 `url:""+data` 会被静默丢弃；见 `message-v2.ts:170-187`，该路径**绕过** `image.normalize`）；发送时截图另落盘并在文本里给出路径；显式检查 SDK 返回的 `{error}`（不抛异常）并 log+notify；轮询器出错不再 `clearInterval`；发送后**面板列表清空**（仅面板列表为空、不再显示已发送项——与参考实现 `clearRecords()` 语义一致；Rust 侧 `panel_records`/`records-changed` 均按 `!sent` 过滤），**记录本身保留并标记 `sent`**（Rust `records` 不清空；`list_records`/`read_record_content` 走 `/api/annotate/records` 仍能读到全部，含截图）

- [x] 投递前 session id 可靠来源：真实 GUI 取证 `client.session.list()` 返回 `sessions=0` 导致无法投递；改为**优先用插件钩子缓存的 sid**（`Event.properties.sessionID`、`session.created/updated/deleted` 的 `properties.info.id`、`chat.message`/`tool.execute.before`/`shell.env` 入参 `sessionID`、工具 `context.sessionID`），`list()` 仅兜底并打印原始包络；仍拿不到则 log+notify 不静默
- [x] 日志规范（插件侧）：统一用公共包 `@xiaoqiong0v0/opencode-plugin-logger`，项目内 `src/logger.ts` 导出共用实例 `log`；**禁止用 `console.*` 往 stdout/stderr 打**（会污染 opencode TUI），`src/**` 的 `console.*` 仅允许出现在独立 CLI 的 `isMain` 守卫内（`src/binary.ts`）；Rust 的 `println!`/`eprintln!` 默认被 spawn 管道吞掉，必须由插件把子进程 stdout/stderr 转发进 logger 才可见（`src/client.ts` 的 `attachStdio`，client.ts:307-316）

- [x] 面板发送改为**推送式投递**（对齐参考 `opencode-playwright-tool` 的 bridge 机制）：插件本地监听 `127.0.0.1:<随机端口>/notify`（仅接收最小 `{count}`，大内容仍走 drain）；shell `send_all_records` 标记 sent 后 **fire-and-forget** 交给单线程 FIFO worker 投递（首发 + 最多 3 次重试，退避 300/600/1200ms，单次超时 2s，`ui/mod.rs:notify_plugin`/`notify_worker`；失败 `eprintln!` 经插件转发可见）；地址传递：spawn 传 `--notify-url` / 环境变量 `BT_SHELL_NOTIFY_URL` / 附着模式同样通过 `/api/notify-url` 注册到插件通知服务（1.9.1 起；共享 shell 的批注队列为全局，由接收 sid 执行 drain）。原 `startAnnotatePoller`（`setInterval` 2s）**已删除**，改为事件驱动 `drainAnnotate`（`index.ts:606`）：通知即 drain + 服务就绪/插件启动各 drain 一次（覆盖窗口，幂等）；拿不到 sid 时仍 log+notify
- [x] Linux 布局尺寸口径修复（v1.5.0，动 Rust）：`apply_layout` 在 Linux 正常路径改用顶层 GtkWindow `allocation()` 的 **GTK 逻辑尺寸**（新增 `rust/src/ui/linux_layout.rs::logical_window_size`）并以 `layout_scale = 1.0` 计算，取不到时回退 `size`+`scale` 物理口径；修复 GNOME 分数缩放/HiDPI 下子视图按物理值放大、内容区超出窗口（坐标口径=GTK 逻辑像素，见 `linux-support.md` §2）

- [x] 地址栏/标题/图标改**事件驱动**（删除 `main.rs` 的 `poll_tab_title` 1 秒轮询）：导航级 `on_navigation`（请求即更新地址栏）+ `on_page_load`（完成后异步读真实 `location.href`/`title`）；站内 SPA（包装 `pushState`/`replaceState`、`popstate`/`hashchange`）与标题/图标变化由注入脚本经 **HTTP `POST /api/page-event`** 上报。守卫：`is_internal_url`/`is_error_url` 不覆盖 `TabState.url`，错误页置 `errored`。刷新语义：错误页 → 重新导航原 URL（`/api/reload` 与工具栏共用 `ui::reload_active_tab`），否则原生 reload；工具栏导航不再静默（未就绪写日志）
- [x] **元素截图统一行为**（v1.7.0，动 Rust）：`screenshot --selector` 固定为 exists 校验 → `scroll_to_element`（等稳定，未稳定如实带 `stable:false`/`reached:false`）→ **重测 rect** → 完全在视口内走常规 clip / 仍超出视口走**越界路径整块截全**（Windows `captureBeyondViewport:true`；Linux `SnapshotRegion::FullDocument` + cairo 文档坐标裁剪；无尺寸上限）。坐标系实测：**CDP `clip` 恒为文档坐标**（与 `captureBeyondViewport` 取值无关），截图层统一做视口坐标+`scrollX/Y` 换算（顺带修复旧版「滚动页面上元素截图截错区域」隐患，`ui/annotate.rs` 区域截图同受益）。WSLg 实测：Linux `FullDocument` 原点=文档 (0,0)、`Visible` 原点=视口；4 用例（普通/超高 3000px/折下懒加载/错误口径）+ 无 selector 回归两端通过；sticky/fixed 在越界路径只成像一次且两端位置不同（见 `linux-support.md` §4）
- [x] **内容区尺寸语义 + 窄窗口适配**（v1.8.0，动 Rust + 前端）：`resize`/`set_device` 宽高一律指**页面内容区（视口）**，窗口高 = 内容高 + `TOOLBAR_HEIGHT`(72)；`resize` clamp 到 `CONTENT_MIN_SIZE`(360) 并返回 `clamped`，窗口 `min_inner_size` 改为 (360, 432)（程序化 `set_size` 不受 min_inner_size 约束，必须 clamp 在调用侧做）；`apply_layout` 窄模式（窗口**逻辑**宽 < `NARROW_MODE_MAX_W`=480，Windows 物理宽需除以 scale）面板 `panel_w = win_w` 填满（x=0；页面 webview 保持全宽在下层，关闭面板即见页面）；工具栏批注/截图/面板按钮文字包 `span.btn-label` + `@media (max-width: 479px)` 隐藏（裸文本节点 CSS 无法隐藏；媒体查询按工具栏 webview 的 CSS 视口=窗口逻辑宽，与 Rust 阈值同步维护）。z-order：面板每次 apply_layout 后由 `bring_webviews_to_top`（Windows SetWindowPos HWND_TOP，panel 最后）置顶；Linux 靠创建顺序（panel 最后建）+ show 时 `linux_layout::raise`

### 关键经验
- **投递批注取 session id**：`client.session.list()` 是 GET，SDK 会追加 `?directory=<插件客户端目录>`（`@opencode-ai/sdk/dist/client.js` 的 `rewrite`），在真实插件进程里实测可能返回空包络（本项目一次真实 GUI 操作为 `sessions=0`），因此**不可作为唯一来源**；可靠来源是插件钩子：`chat.message`/`tool.execute.before`/`shell.env` 入参 `sessionID`、工具 `context.sessionID`、以及 `Event.properties.sessionID`——注意 `session.created/updated/deleted` 的 id 在 `properties.info.id`（不是 `properties.sessionID`），旧代码读 `event.data.sessionID` 是错的。实现见 `src/session-id.ts`
- **日志降噪**：`/api/annotate/consume-sent`（`rust/src/service.rs`）仅在**非空**时 `eprintln!`；投递已是推送式（`/notify` 到达即 drain），不存在周期性空拉取；插件 drain 也只在非空/出错时记录
- **页面→Rust 事件通道**：远程页面里 `window.__TAURI_INTERNALS__.invoke` **存在但恒 pending**（Tauri 能力门禁，实测），不可用；改用注入脚本 `fetch(..., {mode:"no-cors"})` POST 到 shell 自身 `http://127.0.0.1:<port>/api/page-event`（该路径在 `http.rs` 里按精确路径豁免鉴权，`no-cors` 免预检、错误页/不透明源也能发）。端口/label 由 Rust 逐 webview 注入（`window.__btEventPort`/`__btPageLabel`）。**注意**：`on_page_load` 即使是错误页也只给**请求 URL**（`http://nonexistent.invalid/`），真实错误地址 `chrome-error://…` 只能"加载后 `eval(location.href)`"读到——故错误检测放在加载完成后的异步读取里
- `eval_with_callback` 自动 JSON 序列化 JS 返回值，表达式直接返回对象（勿双重 stringify）
- **opencode 宿主对 tool 返回值要求字符串**：命令 `run` 返回对象会让宿主（Bun/JSC）报 `undefined is not an object (evaluating 'c.split')`；结构化结果请 `JSON.stringify` 或自行格式化（截图类返回 image attachment 属例外）
- **Windows 可访问性树（CDP）**：`Accessibility.getPartialAXTree` 即使 `fetchRelatives=false` 也只返回命中节点自身（其 `parentId` 指向不在结果内的祖先）→ 取"某选择器子树"应改用 `DOM.getDocument`+`DOM.querySelector`+`DOM.describeNode`(取 backendNodeId) 定位命中 AX 节点，再用 `Accessibility.getFullAXTree` 并以该节点为根建树；CDP 会把文本拆成逐字符 `StaticText` + `InlineTextBox`，且 `html`/`body`/无名容器为 ignored → 需丢弃 InlineTextBox、把 StaticText 文本合并进父节点 name、并按"name 值为空"跳过无名 generic（原判据 `name` 字段缺失对 CDP 恒 false）
- **CDP `Page.captureScreenshot` 的 `clip` 恒为文档坐标**（v1.7.0 实测，`captureBeyondViewport` true/false 均如此）：24px 黄带元素居中于 scrollY≈645 后按视口坐标 (y=302) 传 clip → 截到的是文档 y302..326 的空白区（白图）；按文档坐标 946 传 → 黄带 ✓。因此视口坐标必须 +`scrollX/Y` 再传 CDP；`captureBeyondViewport:false` 只是把截取限制在视口表面（clip 须完全落在可见视口内），`:true` 才能超出视口整块截全（3000px 元素 → 高度=rect.h ✓）
- release 模式 Tauri GUI 无控制台，HTTP 端口需 `--port` 显式指定 + 轮询就绪
- 覆盖层 show/hide 控制鼠标拦截（批注模式拦截，普通模式穿透）
- Windows 必须显式 `SetProcessDpiAwarenessContext(PER_MONITOR_AWARE_V2)`，否则窗口被虚拟化缩放与 WebView2 布局错位
- 截图平台分支：Windows CDP（webview2-com 版本须与 tauri 对齐 0.38.2）；Linux WebKitGTK `WebView::snapshot` 只有 `Visible`/`FullDocument` 两种区域、**无矩形参数**，clip 需先截 Visible 再用 cairo 偏移裁剪（`set_source_surface` + `paint`）；`Surface::write_to_png` 位于 cairo-rs 的 `png` feature 下而 gtk 0.18 未启用，故 Linux target 需显式加 `cairo-rs = { version = "0.18", features = ["png"] }`；截图调用方都在后台线程，`with_webview` 在非主线程仅投递消息到 GTK 主线程，主线程阻塞 `recv_timeout` 会死锁、后台线程不会
- 动态创建的 page webview 会盖在 overlay/panel 之上 → Windows SetWindowPos(HWND_TOP) 置顶
- 页面 file:// 的 tauri IPC emit 被拒（Origin 非法）→ 新窗口拦截用 WebView2 原生事件而非页面 JS
- AX 树解析：过滤 generic 容器自身但透传子树，否则只剩根节点
- Mutex 加锁注意提前 drop，避免同一线程重入死锁（emit_mode_state 曾卡死 HTTP 服务）
- 前端 build 的 copyHtml 可能不更新 dist HTML（esbuild 缓存）→ 需手动 Copy-Item src/*.html dist/
- 调试注意：非 DPI 感知进程读取 GetWindowRect 得到虚拟化坐标，截图会错位
- **二进制分发/下载（`src/binary.ts`）**：首次使用后台从 GitHub Release 下载到 `~/.opencode/plugins-data/opencode-browser-tool/<triple>/bt-shell[.exe]`（**固定路径就地覆盖**，不按插件版本建目录；旁文件 `bt-shell.version.json` 记录来源 `tag/asset/sha256/size/downloadedAt/sourceUrl`，避免按版本累积与"二进制没变却因插件升版重下"）；**幂等**（已存在且 sha256 与当前插件版本对应 release 的 `SHA256SUMS` 一致即跳过，**每次都拉 SHA256SUMS 校验、取不到即失败**）、**原子**（下到 `<target>.part`，校验通过后 `rename` 替换）、**并发**（同进程共享 Promise；跨进程 `<target>.lock` 原子创建、陈旧锁可接管、抢不到则等待复用）、**有界**（`ensureBinary(timeoutMs)` 不无限等待）；下载优先用系统 `curl`（自动遵循 `HTTPS_PROXY`/`HTTP_PROXY`/`NO_PROXY`），无 curl 时回退 Node `fetch`；启动时清理旧布局版本目录（`<pluginDataDir>/<x.y.z>/`，dry-run 列清单，可按 sha 移动合法资产省一次下载）。二进制解析优先级（`src/client.ts` 的 `resolveShellBinary`，选中的路径与来源会记入日志 `Shell binary: <path> (source=env|local-dev|cargo-target-dir|download-cache)`）：`BT_SHELL_PATH` → **opencode 应用目录**（`PluginInput.directory` / 工具 `context.directory`）下的 `rust/target/release`、`target/release`、`../rust/target/release`（含 debug）→ 源码树 `<install>/../rust/...` → `CARGO_TARGET_DIR` → `<install>/bin/...` → 下载缓存。**动机**：用 `.tmp/publish-local.ps1` 覆盖到 `~/.cache/opencode/packages/...` 后，插件 dist 的相对路径不再指向仓库，必须靠 opencode 传入的 directory 才能命中本地 Rust 构建（否则会错误地回落下载缓存）。**版本策略**：二进制产物只挂 `x.Y.0` 的 Release（动 Rust 升 minor 并发新 exe；纯插件改动只升 patch、复用同一二进制），插件由自身版本推导二进制 tag（`binaryReleaseVersion`：patch 归 0、忽略预发布/构建后缀，如 `1.2.1→1.2.0`、`1.3.0-beta.1→1.3.0`）；`BT_SHELL_VERSION` 表示显式二进制发布版本（形如 `1.2.0`）、原样使用不推导；旁文件同时记 `tag`（推导后）与 `pluginVersion`（插件自身版本）
- **插件持有 exe 文件锁 → 部署新二进制必须先退出 opencode**：opencode 启动时 spawn `rust/target/release/bt-shell.exe`，运行期间该文件被锁，此时 `cargo build --release` 会在最后一步报 `failed to remove file ... (os error 5)`（编译已完成，只是拷贝被拒），**磁盘上的产物不可信任**。部署顺序：退出 opencode（确认 shell 也已退出）→ 在 `rust/` 执行 `cargo build --release` → 重新启动 opencode（插件用它拉起 shell）；构建失败后不要依赖旧产物，退出 opencode 后重跑构建。同理，插件 TS（`dist/`）的改动也需要重启 opencode 才会生效

## 会话隔离架构(1.9.0,用户开发规范)

**规范来源**:用户开发规范「每个 opencode 会话持有自己的 bt-shell 实例」。1.9.0 批次 5 落地;此前(≤1.8.x)是「一 opencode 进程 = 一个共享 shell」,并发会话共用窗口/标签/UA/批注,互相踩(审计见 `.tmp/scratch/isolation-audit.md`)。

### 实例注册表

- **数据结构**:`Map<sid, ShellInstance>`(`src/client.ts:127`);`ShellInstance`(`client.ts:106`)持有 `sid / proc / baseUrl / port / token / dataDir / title / ready / attach`。启动中的会话另存 `Map<sid, Promise>` 做并发保护(`client.ts:129`),同一 sid 多个工具同时调用只 spawn 一次。
- **sid 来源**(`resolveSessionId`,`client.ts:194`):工具调用的显式 `_sessionId` → 请求体 `_sessionId` → 钩子缓存兜底(`setSessionIdResolver`,`client.ts:176`;`index.ts:330` 指向 `sessionCache.get()`)→ `"default"` 桶。`index.ts` 的 `sidOf`/`sidArgs`(`index.ts:28,33`)把每条命令的 sid 塞进请求体。
- **路由**:`callApi`(`client.ts:757`)按 sid 走 `ensureInstance`(`client.ts:431`) 后打到该实例的 `baseUrl`;**旧的模块级单例(`serviceProcess/serviceReady/serviceBaseUrl`)已删除**。

### 数据目录

- 每会话独立 WebView2 profile:`getSessionDataDir(sid)` = `<cacheDir>/sessions/<profile>/<sid>`(`config/index.ts:126`)→ 传给该实例的 `--user-data-dir`,cookie/localStorage/缓存全隔离。实测目录内出现 `EBWebView` 即 WebView2 真实在用。
- 会话临时产物(截图落盘)走 `getSessionTmpDir(sid)` = `<...>/<sid>/tmp`(`config/index.ts:131`,`index.ts:512`),不再共享全局 `tmp/`。
- **路径取舍**:需求字面为 `sessions/<sid>`;实现多一层 `<profile>` 是为了保住 `profile --set` 的语义(见下)。要严格字面路径,去掉 `getSessionsDir` 里的 profile 段即可。

### 生命周期

- **懒启动**:`ensureInstance(sid)`(`client.ts:431`)在首次工具调用时 spawn;**端口重试**(`client.ts:348-398`,上限 `PORT_START_ATTEMPTS=5`,`client.ts:25`):启动超时(常见于端口被占 → bind 失败)则只杀掉**本次刚 spawn 的**子进程、换随机端口重来,5 次全失败才报错。实测占住 18000-18200 共 201 端口后仍成功落在 18766。
- **关闭**:`closeSession(sid)`(`client.ts:582`)= `/api/close` 优雅 → `terminateOwnProcess`(`client.ts:292`)等退出(有界 5s)→ 超时才 SIGTERM。**只按注册表里的句柄/PID,绝不按名杀**;实例不存在直接返回(**绝不因 close 而 spawn**)。
- **关闭后不自动重开(1.9.1)**:`closeSession` 同时置 `closedSids`(`client.ts:585`);`ensureInstance` 对已关会话直接抛 `client.browser_closed` 指引(`client.ts:436-438`);手动关窗/进程被杀这类连接层错误由 `callApi` 捕获 → `markInstanceDead`(`client.ts:726`)+置关闭态(`client.ts:772-776`),**绝不 respawn**。唯一打开入口 `openWindow`(`client.ts:639`,先探测死实例、再以 `allowClosed` 重开,`client.ts:656`);`bt_close` 对启动中的实例也生效(`isStarting`,`client.ts:627`)。
- **会话删除**:`session.deleted` 事件 → 清钩子缓存 + `closeSession(delId)`(`index.ts:350-363`)。语义升级为「回收该会话的浏览器」,但仍是「没有实例就不动进程」。
- **插件卸载**:`dispose` 钩子 → `closeAllSessions()`(`client.ts:616`,`index.ts:461`)。
- **未做**(可选后续):idle TTL 回收(会话长时间无操作后自动关浏览器)。

### 投递归属

- 每个 shell 实例注册**自己**的 `--notify-url` = `<插件监听端口>/notify/<sid>`(`notify-server.ts:68`;spawn 时传参 `client.ts:365`,就绪后再 `registerNotifyUrl`(`client.ts:511`) 兜底注册)。
- 附着模式共享一个外部 shell(队列全局):每个 sid 首次连接都按 `baseUrl+sid` 去重注册 notify(`ensureAttachNotify`,`client.ts:535`),避免重复 POST。
- 通知到达 → `notify-server` 从 URL 提取 sid(`notify-server.ts:27-38`)→ 只 drain **该会话**(`index.ts:598` → `drainAnnotate(sid)`,`index.ts:606`)。彻底取代旧的「最后活跃会话」(last-writer-wins)。
- 实例就绪回调同样带 sid(`setServiceReadyHandler`,`client.ts:186`;`index.ts:593`)。

### 窗口标题

- spawn 时传 `--title "bt-shell · <会话标识>"`;Rust `main.rs:66` 解析、`ui/mod.rs:738` 应用(`title.unwrap_or("bt-shell")`),任务栏可分辨多实例。
- 标识优先用会话标题(`setTitleResolver`,`client.ts:181`;`index.ts:332` 经 `client.session.get`;**250ms 有界超时**由 `client.ts:218-232` 的 `Promise.race` 实现,上限常量 `client.ts:27`),拿不到回退 sid 后 6 位,不阻塞启动。
- `/api/title`(`service.rs:157,234`)返回主窗口标题,供自验/诊断。

### 附着模式边界

- `BT_SHELL_URL`/`BT_SHELL_PORT` 存在时(`client.ts` `resolveAttachBaseUrl`)→ 连接外部单例,**所有 sid 共用、不 spawn、不 kill、放弃会话隔离**(`attachToService`,`client.ts:402`)。假地址时明确报错且列表为空(自验已测)。

### 死参数清理

- **已删除（1.9.0）**：`--browser` / `--session-isolation` 参数不再存在（Rust `main.rs` 本就不解析）；`PluginConfig` 移除 `browserType`/`sessionIsolation`(`config/index.ts:27`)。旧配置文件里若仍写着这两项,`applyConfigFile` 自然忽略(`config/index.ts:202`)。**会话隔离已改为默认行为，不可再配置。**
- **每会话独立实例现在是唯一行为**,不存在「开关」。

### profile 作用域变更

- `profile --set` 现在只写全局 `active-profile` 文件,影响**后续新会话**的数据根(`sessions/<profile>/<sid>`);**不再重启 shell**、运行中的实例不受影响(`index.ts` profile 分支 + i18n `cli.out.profile_switched`)。

### 已知代价

- **内存随会话数增长**:每会话一个 WebView2 进程组(主进程 + GPU + 渲染 + 实用工具进程),约数十 MB~百 MB 量级。并发会话多时需关注任务管理器。缓解手段(未实现):idle TTL 回收、会话数上限。

## 本地 API 鉴权（1.10.0）

**动机**：HTTP API 此前只绑 `127.0.0.1` 且返回 `Access-Control-Allow-Origin: *`，本机任何网页都能跨源调用（读页面内容、驱动浏览器）。1.10.0 起改为「回环 + 共享令牌 + 拒绝浏览器来源」。

### 令牌来源与生命周期

- 优先级：命令行 `--token <hex>` → 环境变量 `BT_SHELL_TOKEN` → 随机生成（16 字节 → 32 hex）。实现见 `rust/src/main.rs` 的 `resolve_auth_token` / `random_token`（不引入新依赖：`RandomState` 由 OS 熵初始化）。
- 令牌在**实例创建期确定，随实例不可变**；`http::serve(app, port, token)` 把令牌放进 `HttpState`（`rust/src/http.rs` 的 `HttpState`），运行期没有改写入口。
- 落盘：提供了 `--user-data-dir` 时写到 `<user-data-dir>/bt-auth-token`（目录不存在则创建；Unix 下 `chmod 600`，失败忽略），仅供排查；未提供则不写。见 `persist_auth_token`（`rust/src/main.rs:130`）。
- 日志纪律：只打印 `source / prefix(前 4 位) / len`，令牌长度 ≤ 4 时前缀整体打码；**绝不打印完整令牌**（`token_prefix`，`rust/src/main.rs:121`）。

### 请求校验（`rust/src/http.rs`）

- 所有 `/api/*` 请求必须带请求头 `x-bt-token`；缺失或不匹配 → `401` + JSON `{success:false,error:"unauthorized"}`（不回显期望值）。
- 比较用自实现的常量时间比较 `constant_time_eq`（遍历到较长者、长度不等直接落入非零 diff，无提前返回）。
- **除 `/api/page-event` 外，任何带 `Origin` 请求头的请求一律 `403`**（浏览器跨源必带，插件 Node `fetch` 不带）——这是与令牌并列的第二道防线，先于 `OPTIONS` 判定。
- **移除全部 CORS 响应头**（`Access-Control-Allow-Origin` 等），`OPTIONS` 直接 `405` + `{error:"method_not_allowed"}`。
- **唯一豁免：精确路径 `/api/page-event`**（非前缀匹配，`/api/page-event/extra` 仍 401）。页面注入脚本运行在远程页面源下、拿不到令牌，浏览器又必带 `Origin`，故该路径跳过 `Origin` 与令牌两项校验后照常分发。残余风险：该端点只把 url/title/icon 写进 `TabState` 提示、不执行操作、无敏感回读，被同机页面伪造最多导致地址栏/标题错乱；若未来给它加敏感字段或副作用，必须重新评估豁免。
- 校验顺序：`page-event` 精确判定 → `Origin`（豁免路径跳过）→ `OPTIONS` → 令牌（豁免路径跳过）→ 读 body → `App::handle`。

### TS 侧（`src/client.ts`）

- `ShellInstance` 新增 `token`（`client.ts:116`），所有 `/api/*` 调用统一经 `authHeaders(token)`（`client.ts:94`）带 `x-bt-token`：`postJson`、`probeOnce`、`waitForReady`、`registerNotifyUrl`、`closeSession` 的 `/api/close`。
- **启动模式**：`spawnForSession` 生成 `randomBytes(16).toString("hex")`（`client.ts:346`），以 `--token` 传给 shell；端口重试沿用同一令牌。
- **附着模式**：令牌取 `process.env.BT_SHELL_TOKEN`（`requireAttachToken`，`client.ts:99`），为空直接抛 `client.attach_need_token`（i18n zh/en 同键），不去连注定 401 的端点；令牌错误（401/403）是**确定性失败**，`waitForReady(..., authHint=true)` 立即抛 `client.attach_bad_token`，不等 20s 就绪超时。外部 shell 必须用同一令牌启动（`--token` 或同值环境变量）。
- 纯查询 `probeStatus` 改为按实例探测（每个实例带自己的令牌，attach 实例用环境变量值），`get_browser_status` 仍是纯查询、不 spawn。

### 注意 / 已知边界

- **页面 webview 的 `/api/page-event` 上报通道**（`rust/src/ui/mod.rs` 注入脚本，跨源 beacon 必带 `Origin` 且无法持有令牌）：服务端按**精确路径**豁免校验后恢复可用（见上），SPA 路由/标题/图标的即时上报不再退化。注：代码侧「http.rs 已开 CORS」的过时注释（`ui/mod.rs` 已更正，`service.rs` 的 `page_event` 于本次清理更正）均已改为准确表述——CORS 与 OPTIONS 已移除，`/api/page-event` 为唯一精确路径豁免。
- **面板 Webview 的 `/api/*` 调用**：通过 Tauri 命令 `api_token` 读取本实例令牌（`rust/src/main.rs` 的 `ApiToken` 受管状态，与 `http::serve` 同源），前端启动后首次 `api()` 时缓存并带 `x-bt-token`；读取失败只打面板控制台错误、不阻断面板，令牌不写入 DOM/HTML/日志。
- 令牌是**单实例共享秘密**，不做多用户/多会话区分；附着模式下所有 sid 共用同一个外部 shell 与同一令牌。

## 1.10.0 其他行为（队列 / 记录 / 标签 / DevTools / Linux 截图）

### 待发送队列

- 面板「发送」把本次新标记为 `sent` 的记录**追加**进 shell 侧队列（`append_sent_records`，`ui/mod.rs:1450`）：按记录 `index` 去重（同一批重复 send 不会重复入队），上限 `SENT_QUEUE_MAX=500`，超出丢最旧。旧实现是覆盖写——插件上一批还没 drain 完就再点发送，未消费的记录会被整批丢掉。
- 队列由**插件** drain 消费（通知 / 服务就绪各触发一次，幂等）；shell 只负责追加与通知，见上方投递小节。

### 批注 / 截图记录

- 记录表上限 `MAX_RECORDS=200`（`annotate.rs:395`），超出丢最旧；`index` 单调递增（不按 `len` 推算，避免丢旧记录后与队列里的旧 index 重号）。
- `/api/annotate/records` 的 `includeImages` **默认 false**：列表只返回 `hasImage:boolean`，不带 PNG base64；确需图片时显式传 `includeImages:true`（`read_record_content` 才这么做），避免整批大图塞进响应。
- 发送后记录**仍保留**（只标 `sent`，面板列表按未发送过滤），但可被后续新记录按 200 上限挤出。

### 标签关闭与 webview 生命周期

- 关闭标签会**释放该标签的页面实例**：预创建池（`page-1..page-3`，`MAX_TABS=3`）的 webview 回池（仅隐藏待复用），池外动态创建的 webview 真正 `close()` 销毁（`ui/mod.rs:release_tab_webview`）。存活页面实例 ≈ **池 3 个 + 打开中的动态标签**，反复开关标签不再累积页面进程（1.10.0 前只 `hide()`，内存无界增长）。

### DevTools 平台差异

- `/api/devtools` 固定返回 `{open, supported}`。Windows（WebView2 + wry 0.55.1）的 `close_devtools()` 是空实现、且 WebView2 没有关闭 API：close / toggle-close 保持 `open:true` 并返回 `supported:false`，插件侧据此输出 `cli.out.devtools_unsupported`（提示用 F12 手动切换），不再谎报「已关闭」；Linux（webkitgtk）支持真实关闭，按 `open` 输出开/关文案。

### Linux 截图坐标换算

- Linux 截图裁剪把 CSS 像素 rect 按 `scale_factor` 换算成设备像素（`rust/src/control/screenshot.rs:230,293`，`round(css × scale)`）；HiDPI 下选区不再错位/裁小。Windows 走 CDP `clip`（`scale:1`、CSS 像素口径），见「元素截图行为」。

## 待办

- macOS：截图（WKWebView snapshot）、可信输入（无 CDP/GDK 通道，现走降级或明确报错）等平台分支
- Linux：`upload_file` / `drag` 的可信通道未确证（当前**明确报错**，不假成功）；可访问性树为 DOM 近似
- 真实 Linux 桌面（GNOME/KDE）复验（WSLg 已通过 15 项，见 `linux-verification.md`）
