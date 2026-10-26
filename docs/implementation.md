# 实施状态

## 当前架构（V5：跨平台 Tauri 2 多 Webview）

**当前版本**：插件 **1.11.1**（`package.json`）/ bt-shell **0.11.0**（`rust/Cargo.toml`，未变）。1.10.1 只动插件 TS：`httpCall` 统一原语、模型可见文本脱敏、状态文案去重；1.10.2 为文档一致性 + `read_record_content` 传 `includeImages:true` + Linux 复验脚本补令牌（零 Rust 改动，复用 1.10.0 二进制）。**1.11.0 动 Rust（→ 新二进制 `0.11.0`）**：修 1.10.0 引入的回归——面板 webview 的 `fetch` 带 `Origin: http://tauri.localhost`，被「带 `Origin` 一律 403」挡死，面板 `/api/device/list` 取不到、设备预设下拉只剩「默认」；现改为面板 origin 白名单 + 最小 CORS（预检 204/精确 ACAO），其它 Origin 仍 403。**1.11.1 仅插件 TS 改动**（零 Rust 改动，复用 0.11.0 二进制）：会话树归并——task 子代理会话默认归并到根会话、新增全局 flag `--private-session`、i18n 新增 `cli.usage.flags` 与 `cli.err.flag_not_number`（键数 168 → 170，zh/en parity）；修复数值 flag 未转 number 导致 `resize`/`switch_tab --index`/`fake_audio --freq/--durMs` 静默失效。本文所有 `src/client.ts` 行号按 **1.10.2** 代码核对（1.11.0/1.11.1 未改 `client.ts`），`src/index.ts` 行号按 **1.11.1** 代码核对，`rust/src/http.rs` 行号按 **0.11.0**。

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
- [x] 纯查询命令不懒启动：`get_browser_status` 走 `probeStatus()`（`client.ts:642`，短超时 HTTP 探测，**不 spawn**）；未运行时返回未运行 + bt-shell 二进制状态
- [x] 发送（批注/截图）修复（以旧 `opencode-playwright-tool` 为参考）：面板"发送"**只发文本**（与参考一致；`session.prompt` 的 parts 不支持图片 —— `image/*` 走 `image.normalize` 返回 400，参考实现从不这么发）；截图图片走**工具结果附件**交付：`attachments:[{type:"file", mime:"image/png", url:"data:image/png;base64,…"}]`（关键：opencode 只接受 `url` 以 `data:` 开头且含 `,` 的附件，旧形状 `url:""+data` 会被静默丢弃；见 `message-v2.ts:170-187`，该路径**绕过** `image.normalize`）；发送时截图另落盘并在文本里给出路径；显式检查 SDK 返回的 `{error}`（不抛异常）并 log+notify；发送后**面板列表清空**（仅面板列表为空、不再显示已发送项——与参考实现 `clearRecords()` 语义一致；Rust 侧 `panel_records`/`records-changed` 均按 `!sent` 过滤），**记录本身保留并标记 `sent`**（Rust `records` 不清空）。图片语义分两条通道：`list_records` 只拿记录列表、响应仅给 `hasImage`（**不回 base64**，`rust/src/service.rs:1294-1300`）；要图须由 `read_record_content` 显式传 `includeImages:true` 取回 base64 并落盘（`src/index.ts:206`）

- [x] 投递前 session id 可靠来源：真实 GUI 取证 `client.session.list()` 返回 `sessions=0` 导致无法投递；改为**只用插件钩子 / 事件缓存的 sid**（`Event.properties.sessionID`、`session.created/updated/deleted` 的 `properties.info.id`、`chat.message`/`tool.execute.before` 入参 `sessionID`、工具 `context.sessionID`），实现见 `src/session-id.ts`；`client.session.list()` **已不再调用**（仅 `session-id.ts:3,22` 注释留此教训）。**当前兜底是静默的**：`drainAnnotate` 拿到空 sid 直接 `return`（`src/index.ts:668`，无 log/notify），`resolveSessionId` 兜底 `"default"` 桶（`src/client.ts:199`）
- [x] 日志规范（插件侧）：统一用公共包 `@xiaoqiong0v0/opencode-plugin-logger`，项目内 `src/logger.ts` 导出共用实例 `log`；**禁止用 `console.*` 往 stdout/stderr 打**（会污染 opencode TUI），`src/**` 的 `console.*` 仅允许出现在独立 CLI 的 `isMain` 守卫内（`src/binary.ts`）；Rust 的 `println!`/`eprintln!` 默认被 spawn 管道吞掉，必须由插件把子进程 stdout/stderr 转发进 logger 才可见（`src/client.ts` 的 `attachStdio`，client.ts:293-302）

- [x] 面板发送改为**推送式投递**（对齐参考 `opencode-playwright-tool` 的 bridge 机制）：插件本地监听 `127.0.0.1:<随机端口>/notify/<sid>`（仅接收最小 `{count}`，大内容仍走 drain）；shell `send_all_records` 标记 sent 后 **fire-and-forget** 交给单线程 FIFO worker 投递（首发 + 最多 3 次重试，退避 300/600/1200ms，单次超时 2s，`ui/mod.rs:notify_plugin`/`notify_worker`；失败 `eprintln!` 经插件转发可见）；地址传递：spawn 传 `--notify-url` / 环境变量 `BT_SHELL_NOTIFY_URL` / 附着模式同样通过 `/api/notify-url` 注册到插件通知服务（1.9.1 起；共享 shell 的批注队列为全局，由接收 sid 执行 drain）。旧的定时轮询器（`startAnnotatePoller`）**已删除**，改为事件驱动 `drainAnnotate`（`index.ts:667`）：通知即 drain + 服务就绪/插件启动各 drain 一次（覆盖窗口，幂等）；**空 sid 时直接 `return`（`index.ts:668`，不 log/notify）**
- [x] Linux 布局尺寸口径修复（v1.5.0，动 Rust）：`apply_layout` 在 Linux 正常路径改用顶层 GtkWindow `allocation()` 的 **GTK 逻辑尺寸**（新增 `rust/src/ui/linux_layout.rs::logical_window_size`）并以 `layout_scale = 1.0` 计算，取不到时回退 `size`+`scale` 物理口径；修复 GNOME 分数缩放/HiDPI 下子视图按物理值放大、内容区超出窗口（坐标口径=GTK 逻辑像素，见 `linux-support.md` §2）

- [x] 地址栏/标题/图标改**事件驱动**（删除 `main.rs` 的 `poll_tab_title` 1 秒轮询）：导航级 `on_navigation`（请求即更新地址栏）+ `on_page_load`（完成后异步读真实 `location.href`/`title`）；站内 SPA（包装 `pushState`/`replaceState`、`popstate`/`hashchange`）与标题/图标变化由注入脚本经 **HTTP `POST /api/page-event`** 上报。守卫：`is_internal_url`/`is_error_url` 不覆盖 `TabState.url`，错误页置 `errored`。刷新语义：错误页 → 重新导航原 URL（`/api/reload` 与工具栏共用 `ui::reload_active_tab`），否则原生 reload；工具栏导航不再静默（未就绪写日志）
- [x] **元素截图统一行为**（v1.7.0，动 Rust）：`screenshot --selector` 固定为 exists 校验 → `scroll_to_element`（等稳定，未稳定如实带 `stable:false`/`reached:false`）→ **重测 rect** → 完全在视口内走常规 clip / 仍超出视口走**越界路径整块截全**（Windows `captureBeyondViewport:true`；Linux `SnapshotRegion::FullDocument` + cairo 文档坐标裁剪；无尺寸上限）。坐标系实测：**CDP `clip` 恒为文档坐标**（与 `captureBeyondViewport` 取值无关），截图层统一做视口坐标+`scrollX/Y` 换算（顺带修复旧版「滚动页面上元素截图截错区域」隐患，`ui/annotate.rs` 区域截图同受益）。WSLg 实测：Linux `FullDocument` 原点=文档 (0,0)、`Visible` 原点=视口；4 用例（普通/超高 3000px/折下懒加载/错误口径）+ 无 selector 回归两端通过；sticky/fixed 在越界路径只成像一次且两端位置不同（见 `linux-support.md` §4）
- [x] **内容区尺寸语义 + 窄窗口适配**（v1.8.0，动 Rust + 前端）：`resize`/`set_device` 宽高一律指**页面内容区（视口）**，窗口高 = 内容高 + `TOOLBAR_HEIGHT`(72)；`resize` clamp 到 `CONTENT_MIN_SIZE`(360) 并返回 `clamped`，窗口 `min_inner_size` 改为 (360, 432)（程序化 `set_size` 不受 min_inner_size 约束，必须 clamp 在调用侧做）；`apply_layout` 窄模式（窗口**逻辑**宽 < `NARROW_MODE_MAX_W`=480，Windows 物理宽需除以 scale）面板 `panel_w = win_w` 填满（x=0；页面 webview 保持全宽在下层，关闭面板即见页面）；工具栏批注/截图/面板按钮文字包 `span.btn-label` + `@media (max-width: 479px)` 隐藏（裸文本节点 CSS 无法隐藏；媒体查询按工具栏 webview 的 CSS 视口=窗口逻辑宽，与 Rust 阈值同步维护）。z-order：面板每次 apply_layout 后由 `bring_webviews_to_top`（Windows SetWindowPos HWND_TOP，panel 最后）置顶；Linux 靠创建顺序（panel 最后建）+ show 时 `linux_layout::raise`

### 关键经验
- **投递批注取 session id**：`client.session.list()` 是 GET，SDK 会追加 `?directory=<插件客户端目录>`（`@opencode-ai/sdk/dist/client.js` 的 `rewrite`），在真实插件进程里实测可能返回空包络（本项目一次真实 GUI 操作为 `sessions=0`），因此**已彻底弃用**（当前代码无任何 `list()` 调用，取标题只用 `client.session.get`，`src/index.ts:351`）；可靠来源是插件钩子与事件：`chat.message`（`src/index.ts:399`）/`tool.execute.before`（`src/index.ts:402`）入参 `sessionID`、工具 `context.sessionID`（`src/index.ts:475` 统一注入 `params._sessionId`）、以及 `Event.properties.sessionID`——注意 `session.created/updated/deleted` 的 id 在 `properties.info.id`（不是 `properties.sessionID`），旧代码读 `event.data.sessionID` 是错的。**没有 `shell.env` 钩子**（曾以为有，实际只注册上面两个）。实现见 `src/session-id.ts`
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

**规范来源**:用户开发规范「每个 opencode 会话持有自己的 bt-shell 实例」。1.9.0 批次 5 落地;此前(≤1.8.x)是「一 opencode 进程 = 一个共享 shell」,并发会话共用窗口/标签/UA/批注,互相踩(审计见 `.tmp/scratch/isolation-audit.md`)。**1.11.1 起规范收窄为「每个根会话持有自己的实例」**:task 子代理会话默认归并到根会话,见下「会话树归并」。

### 实例注册表

- **数据结构**:`Map<sid, ShellInstance>` = `sessions`(`src/client.ts:123`);`ShellInstance`(`client.ts:102`)持有 `sid / proc / baseUrl / port / token / dataDir / title / ready / attach`。启动中的会话另存 `Map<sid, Promise>`(`startingSid`,`client.ts:125`)做并发保护,同一 sid 多个工具同时调用只 spawn 一次。
- **sid 来源**(`resolveSessionId`,`client.ts:190`):工具调用的显式 `_sessionId` → 请求体 `_sessionId` → 钩子缓存兜底(`setSessionIdResolver`,`client.ts:172`;`index.ts:347` 指向 `sessionTree.knownKey(sessionCache.get())`,1.11.1 起返回归并后的 key、未解析时回退 raw)→ `"default"` 桶(`client.ts:199`)。工具 `execute` 统一注入 `params._sessionId`(`index.ts:475`,值是**归并后的** `browserSid`);`sidOf`/`sidArgs`(`index.ts:30,44`)只服务无参命令的按会话路由。
- **路由**:`callApi`(`client.ts:841`)按 sid 走 `ensureInstance`(`client.ts:417`) 后打到该实例的 `baseUrl`;**旧的模块级单例(`serviceProcess/serviceReady/serviceBaseUrl`)已删除**。

### 会话树归并(1.11.1)

**动机**:task 子代理也是 session。若直接用钩子里的 `sessionID` 作路由键,子代理一调 `bt_cli` 就另起一个 shell 与数据目录(多开窗口、登录态互不共享)。归并后主子共用同一实例与同一数据目录。

- **纯逻辑模块** `src/session-tree.ts`(`createSessionTree`,`:62`):不做 IO,父查询由 `fetchParent` 注入。接口:`browserKey(raw)`(`:97`,异步,取最终路由键)、`knownKey(raw)`(`:91`,同步取已知结果,未解析回退 raw)、`markPrivate(raw)`(`:133`)、`isPrivate`(`:138`)、`forget(sid)`(`:142`,清自身缓存 + 私有标记,并清掉所有映射到该 sid 的后代条目)、`cachedRoot`(`:152`,诊断用)。内部两份**进程级**状态:`rootCache: Map<raw, 根sid>`(只缓存成功解析的结果)、`privateSids: Set<raw>`。
- **回溯规则**(`browserKey`):私有会话 → 自身;已缓存 → 缓存值;否则沿 `Session.parentID` 向上——`null` = 确认到根(把**路径上所有节点**一并缓存),`undefined` = 查询失败。单跳超时 `DEFAULT_QUERY_TIMEOUT_MS` = 1500ms(`:49`),最多 `DEFAULT_MAX_HOPS` = 5 跳(`:51`);失败 / 超时 / 成环(`seen` 命中)/ 超限一律**回退 raw 且不缓存**(下次重试),并经 `onWarn`(`index.ts:566` 接到 `log.info`)打日志。根会话自己带 `--private-session` 等价于自身(不报错)。
- **插件落点**(`src/index.ts`):`browserKeyOf(raw)`(`:35`)包一层 try/catch(异常也回退 raw);`bt_cli.execute` 里 `sessionCache.updateFromHookInput(context)` → `rawSid = resolveSessionId(context.sessionID)`(`:440`)→ 带全局 `--private-session` 时先 `sessionTree.markPrivate(rawSid)`(`:441`)→ `browserSid = await browserKeyOf(rawSid)`(`:442`)→ 归并 key 同时用于 `@file:` 允许目录(`fileSid`,`:450`)与 `params._sessionId`(`:475`);`bt_set_media_mode.execute` 同口径(`:493-496`)。`--private-session` 在 `parseFlagArgs` 里预声明为全局 boolean(`:300`,避免被判未知 flag),`execute` 消费后 `delete`(`:435-436`),不进命令业务参数。父查询 client 在插件初始化时赋给 `sessionTreeClient`(`:344`),包络异常/无 `data` 按失败处理(`:557-566`)——绝不把子会话误判为根并长期缓存。
- **`setSessionIdResolver` 为何用 `knownKey` 而非 `browserKey`**(`index.ts:347` ← `client.ts:172`):该解析器是**同步**兜底(请求体没带 `_sessionId` 时才走),不能 await;`knownKey` 返回「已解析的根,否则 raw」,既避免已解析会话被钩子缓存拉回子会话另起实例,又不会在未解析时阻塞——首次解析由 `bt_cli.execute` 的 `browserKeyOf` 完成并写入缓存,之后同步路径自动命中。
- **窗口标题**取**根会话**标题:标题解析器(`setTitleResolver`,`index.ts:349-356`)收到的 sid 就是归并后的 `browserKey`。
- **单测**:`tests/unit/session-tree.test.mjs`(11 项:多级回溯与路径一并缓存、无父即自身、成环回退、超 `maxHops` 回退、超时回退后可重试成功、抛错回退、私有粘住、归并/私有不改 raw 旁路缓存、`knownKey` 同步兜底、`forget` 连带后代、空 sid 落 `default` 桶),`npm test`。

### 数据目录

- 每个**浏览器 key**(`browserKey`,1.11.1 起 = 归并后的根 sid)一份独立 WebView2 profile:`getSessionDataDir(sid)` = `<cacheDir>/sessions/<profile>/<sid>`(`config/index.ts:126`)→ 传给该实例的 `--user-data-dir`,cookie/localStorage/缓存全隔离。子会话与根会话共用 `sessions/<profile>/<根 sid>/`。实测目录内出现 `EBWebView` 即 WebView2 真实在用。
- 会话临时产物(截图落盘)走 `getSessionTmpDir(sid)` = `<...>/<sid>/tmp`(`config/index.ts:131`,落盘函数 `saveShotImage` `index.ts:571`、调用 `getSessionTmpDir` 于 `:573`),不再共享全局 `tmp/`;`@file:` 允许目录同样按归并后的 key 判定(`index.ts:450-452`)。
- **路径取舍**:需求字面为 `sessions/<sid>`;实现多一层 `<profile>` 是为了保住 `profile --set` 的语义(见下)。要严格字面路径,去掉 `getSessionsDir` 里的 profile 段即可。

### 会话数据目录回收(1.9.2,1.11.1 增根/子分支)

- **触发**:`session.deleted` 事件(`src/index.ts:367-395`)。动机:否则会话删了目录还在,磁盘永不释放。**1.11.1 起按归并 key 分支**,不再无条件回收。
- **分支语义**(`index.ts:375-392`):先 `const key = await browserKeyOf(delId)`(`:375`;删除后 `session.get` 可能失败 → 回退 raw,此前工具调用通常已缓存根),再 `sessionCache.clearIf(delId)`(`:376`)、`sessionTree.forget(delId)`(`:377`,清该 sid 的缓存/私有标记及映射到它的后代)),然后分两支:
  - `key === delId`(**根 / 无父 / 私有会话**,`:378-386`):`closeSession` + `clearClosedState` + `clearAttachNotifyForSid` + `removeSessionDataDir(delId)` + `sessionCache.clearIf(key)`——即真正回收实例与数据目录。
  - `key !== delId`(**子会话**,`:387-392`):只 `clearClosedState(delId)` + `clearAttachNotifyForSid(delId)`,**不关根实例、不删根数据目录**(与根共用一份),只打一条 info 日志 `child session deleted sid=… (browserKey=…)`。若误删根目录,同根的其它子会话与主会话会一起丢登录态。
- **绝不因删除而 spawn**:`closeSession` 只在注册表里有该 sid 时才动进程,没有实例直接返回。
- **跨 profile 枚举**:只清当前 profile 会漏(用户切过 `active-profile`),故列举 `getSessionsDir()` 的**直接子目录**(每个 = 一个 profile),逐个尝试删 `sessions/<profile>/<sid>`(`:624-631`)。
- **根路径下的五步顺序**(同一段事件处理内,`index.ts:378-386`):① 清钩子缓存 `sessionCache.clearIf` ② `closeSession(delId)`(只关该会话实例,**绝不 spawn**) ③ `clearClosedState(delId)`(`client.ts:607`,删掉 `closedSids` 记录) ④ `clearAttachNotifyForSid(delId)`(`client.ts:548`,清附着 notify 去重键) → ⑤ `removeSessionDataDir(delId)`。
- **安全约束**:sid 必须匹配 `SESSION_SID_RE`(`index.ts:585`,`^[A-Za-z0-9_-]{1,128}$`),不匹配只记一条错误不删;目标必须在 `getSessionsDir()` 之内,且不等于 sessions 根或 profile 根(`isInsideDir` 判定 `:628`)——**profile 目录本身永不删除**。
- **失败重试**:`rmSessionDir`(`:590-599`)用 `rmSync(recursive,force)`;Windows 文件锁导致失败时对失败列表 `SESSION_DIR_RETRY_MS`=800ms 后重试一次(`:587,633-645`),定时器 `unref()` 不拖住进程退出;仍失败只记错误日志。

### 生命周期

- **懒启动**:`ensureInstance(sid)`(`client.ts:417`)在首次工具调用时 spawn(`spawnForSession` `client.ts:327`);**端口重试**(`client.ts:334-385`,上限 `PORT_START_ATTEMPTS=5`,`client.ts:26`):启动超时(常见于端口被占 → bind 失败)则只杀掉**本次刚 spawn 的**子进程、换随机端口重来,5 次全失败才报错。实测占住 18000-18200 共 201 端口后仍成功落在 18766。
- **关闭**:`closeSession(sid)`(`client.ts:563`)= `/api/close` 优雅(`:583`)→ `terminateOwnProcess`(`client.ts:278`)等退出(有界 5s)→ 超时才 SIGTERM。**只按注册表里的句柄/PID,绝不按名杀**;实例不存在直接返回(**绝不因 close 而 spawn**)。
- **关闭后不自动重开(1.9.1)**:`closeSession` 同时置 `closedSids`(`client.ts:127`,置位于 `:566`);`ensureInstance` 对已关会话直接抛 `client.browser_closed` 指引(`client.ts:422-424`);手动关窗/进程被杀这类连接层错误由 `callApi` 捕获 → `markInstanceDead`(`client.ts:800`)+置关闭态(`client.ts:856-858`),**绝不 respawn**。唯一打开入口 `openWindow`(`client.ts:614`,先探测死实例、再以 `allowClosed` 重开,`client.ts:631`);`bt_close` 对启动中的实例也生效(`isStarting`,`client.ts:602`)。
- **会话删除**:`session.deleted` 事件(`index.ts:367-395`)→ 清钩子缓存 + `sessionTree.forget` + 按归并 key 分支:**子会话**只清自己的关闭标记/附着记录(根实例与根数据目录不动);**根 / 无父 / 私有会话**才 `closeSession` + `clearClosedState` + `clearAttachNotifyForSid` + `removeSessionDataDir`(顺序与约束见上「会话数据目录回收」)。语义升级为「回收该会话的浏览器与数据目录」,但仍是「没有实例就不动进程」。
- **插件卸载**:`dispose` 钩子 → `closeAllSessions()`(`client.ts:591`,`index.ts:504-505`)。注意卸载/进程退出只关实例,**不清 `sessionTree` 缓存**(进程级状态随进程结束一起消失)。
- **未做**(可选后续):idle TTL 回收(会话长时间无操作后自动关浏览器)。

### 关闭态与并发去重状态位

- `closedSids`(`client.ts:127`,`Set<string>`):已请求关闭的会话。**置位只有两处**——`closeSession`(`:566`)与 `callApi` 的连接层错误分支(`:858`,同时调 `markInstanceDead` `:856`);`markInstanceDead` 本身(`:800-807`)只摘注册表句柄/清 attach 记录/终止自己的 PID,**不置位**。清除有 `clearClosedState`(`:607`,会话删除时用)、`openWindow`(`:617`、完成后 `:634`)、以及 `ensureInstance(s,{allowClosed:true})`(`:476`)。**启动链竞态**:spawn 期间若被 close,启动链完成后自查 `closedSids` 并自毁刚起的进程(`:467-469`),且**不删**该标记(用户已表达关闭意图)。
- `startingSid`(`client.ts:125`,`Map<sid, Promise<ShellInstance>>`):同一 sid 并发只 spawn 一次;`isStarting(sid)`(`client.ts:602`)供 `bt_close` 判定「启动中也算有实例可关」;`closeAllSessions`(`:591`)会先置位再等启动链自查自毁。
- `attachNotifyRegistered`(`client.ts:137`,`Set<baseUrl|sid>`)/ `attachNotifyInFlight`(`client.ts:142`,`Map<key, Promise<boolean>>`):附着模式下 notify-url 注册的**去重**与**并发合流**——同一 key 的并发 `ensureAttachNotify`(`:516`)共享同一次 POST(第二个 await 第一个的结果,而不是直接返回);只有成功才写入 `registered`,失败只删 `inFlight` 以便下次重试。清理入口 `clearAttachNotify`(`:537`,按 baseUrl)与 `clearAttachNotifyForSid`(`:548`,按 sid,会话删除时调用)。

### 投递归属

- 每个 shell 实例注册**自己**的 `--notify-url` = `<插件监听端口>/notify/<sid>`(`notify-server.ts:68`;spawn 时传参 `client.ts:351`,就绪后再 `registerNotifyUrl`(`client.ts:497`) 兜底注册;注册失败延迟 300ms 重试一次,`NOTIFY_RETRY_DELAY_MS` `client.ts:145`)。
- 附着模式共享一个外部 shell(队列全局):每个 sid 首次连接都按 `baseUrl|sid` 去重注册 notify(`ensureAttachNotify`,`client.ts:516`),避免重复 POST。
- 通知到达 → `notify-server` 从 URL 提取 sid(`notify-server.ts:27-38`;非法百分号编码降级为空 sid)→ 只 drain **该会话**(`index.ts:659-660` → `drainAnnotate(sid)`,`index.ts:667`)。彻底取代旧的「最后活跃会话」(last-writer-wins)。
- 实例就绪回调同样带 sid(`setServiceReadyHandler`,`client.ts:182`;`index.ts:654` 注册 → 同样走 `drainAnnotate`)。

### 窗口标题

- spawn 时传 `--title "bt-shell · <会话标识>"`;Rust `main.rs:66` 解析、`ui/mod.rs:738` 应用(`title.unwrap_or("bt-shell")`),任务栏可分辨多实例。
- 标识优先用会话标题(`setTitleResolver`,`client.ts:177`;`index.ts:351` 经 `client.session.get`;**250ms 有界超时**由 `client.ts:219-222` 的 `Promise.race` 实现,上限常量 `TITLE_RESOLVE_TIMEOUT_MS` = 250,`client.ts:28`),拿不到回退 sid 后 6 位,不阻塞启动。
- `/api/title`(`service.rs:157,234`)返回主窗口标题,供自验/诊断。

### 附着模式边界

- `BT_SHELL_URL`/`BT_SHELL_PORT` 存在时(`resolveAttachBaseUrl`,`client.ts:86`)→ 连接外部单例,**所有 sid 共用、不 spawn、不 kill、放弃会话隔离**(`attachToService`,`client.ts:388`)。假地址时明确报错且列表为空(自验已测)。

### 死参数清理

- **已删除（1.9.0）**：`--browser` / `--session-isolation` 参数不再存在（Rust `main.rs` 本就不解析）；`PluginConfig` 移除 `browserType`/`sessionIsolation`(`config/index.ts:27`)。旧配置文件里若仍写着这两项,`applyConfigFile` 自然忽略(`config/index.ts:202`)。**会话隔离已改为默认行为，不可再配置。**
- **每会话独立实例现在是唯一行为**,不存在「开关」。

### profile 作用域变更

- `profile --set` 现在只写全局 `active-profile` 文件,影响**后续新会话**的数据根(`sessions/<profile>/<sid>`);**不再重启 shell**、运行中的实例不受影响(`index.ts` profile 分支 + i18n `cli.out.profile_switched`)。

### 已知代价

- **内存随会话数增长**:每会话一个 WebView2 进程组(主进程 + GPU + 渲染 + 实用工具进程),约数十 MB~百 MB 量级。并发会话多时需关注任务管理器。缓解手段(未实现):idle TTL 回收、会话数上限。

## 本地 API 鉴权（1.10.0 / shell 0.10.0 引入，1.10.1 收敛到 `httpCall`，1.11.0 恢复面板最小 CORS）

**动机**：HTTP API 此前只绑 `127.0.0.1` 且返回 `Access-Control-Allow-Origin: *`，本机任何网页都能跨源调用（读页面内容、驱动浏览器）。1.10.0 起改为「回环 + 共享令牌 + 拒绝浏览器来源」。

**1.11.0 修正**：1.10.0 的「带 `Origin` 一律 403」把**自家面板**也挡在门外——面板 webview 的 `fetch` 运行在 `tauri.localhost` 源下，必然带 `Origin`，于是面板所有 `/api/*` 全 403（`/api/device/list` 取不到，设备预设下拉只剩「默认」）。现改为「**面板 origin 白名单 + 最小 CORS**，其它来源仍 403」，令牌闸门不变。

### 令牌来源与生命周期

- 优先级：命令行 `--token <hex>` → 环境变量 `BT_SHELL_TOKEN` → 随机生成（16 字节 → 32 hex）。实现见 `rust/src/main.rs` 的 `resolve_auth_token` / `random_token`（不引入新依赖：`RandomState` 由 OS 熵初始化）。
- 令牌在**实例创建期确定，随实例不可变**；`http::serve(app, port, token)` 把令牌放进 `HttpState`（`rust/src/http.rs` 的 `HttpState`），运行期没有改写入口。
- 落盘：提供了 `--user-data-dir` 时写到 `<user-data-dir>/bt-auth-token`（目录不存在则创建；Unix 下 `chmod 600`，失败忽略），仅供排查；未提供则不写。见 `persist_auth_token`（`rust/src/main.rs:130`）。
- 日志纪律：只打印 `source / prefix(前 4 位) / len`，令牌长度 ≤ 4 时前缀整体打码；**绝不打印完整令牌**（`token_prefix`，`rust/src/main.rs:121`）。

### 请求校验（`rust/src/http.rs`）

- **Origin 规则（1.11.0 起）**：只有**面板自身 origin 白名单**放行，其余任何带 `Origin` 的请求仍 **403**。
  - 白名单 `PANEL_ORIGINS`（`http.rs:40-44`）为**精确字节比较**（`is_panel_origin`，`http.rs:144`，不做大小写/端口/子域归一化）：`http://tauri.localhost`、`https://tauri.localhost`（Windows WebView2 的 tauri 自定义协议源）、`tauri://localhost`（Linux/macOS WebKit 自定义协议源）。
  - `null`、任意 http(s) 站点（`http://evil.com`）、子域变体（`http://tauri.localhost.evil.com`）、带端口（`http://tauri.localhost:3000`）、大写形式（`HTTP://TAURI.LOCALHOST`）**一律 403**。
- **面板最小 CORS**：仅面板 origin 享受 CORS，**永不通配 `*`**、永远原样回显该 origin。
  - 预检 `OPTIONS` → **204**（`preflight_response`，`http.rs:196`）：`Access-Control-Allow-Origin: <该 origin>`、`Access-Control-Allow-Methods: POST, OPTIONS`、`Access-Control-Allow-Headers: content-type, x-bt-token`、`Access-Control-Max-Age: 600`、`Vary: Origin`；若请求带 `Access-Control-Request-Private-Network: true` 再补 `Access-Control-Allow-Private-Network: true`（Chromium 私有网络预检，`http.rs:29-34`）。
  - 实际响应（200，以及面板 origin 的 **401**）也回精确 ACAO + `Vary: Origin`（`attach_cors`，`http.rs:214`；成功路径 `http.rs:140`）。401 带 ACAO 是必需的，否则前端读不到错误体、只会看到 CORS 失败。
  - **非面板 origin / 无 `Origin` 的响应一律不带任何 CORS 头**；非面板 origin（或无 `Origin`）的 `OPTIONS` → `405` + `{error:"method_not_allowed"}`（`method_not_allowed`，`http.rs:259`）。
- 所有非豁免 `/api/*` 必须带请求头 `x-bt-token`（令牌判定在 `decide_auth`，`http.rs:186-188`；取值比较 `token_matches`，`http.rs:227`）；缺失或不匹配 → `401` + `{success:false,error:"unauthorized"}`（`auth_error`，`http.rs:248`，不回显期望值）。比较用自实现的常量时间比较 `constant_time_eq`（`http.rs:235`）。
- **鉴权矩阵（按代码判定顺序，`decide_auth` `http.rs:167-190`）**：

  | 请求形态 | 结果 | 位置 |
  |---|---|---|
  | `POST /api/page-event`（精确路径，含带**任意** `Origin`、无令牌） | **200**（豁免 `Origin` 与令牌两项校验后照常分发） | `http.rs:174-176` |
  | 非豁免路径 + **非面板** `Origin`（无论是否带令牌、是否 `OPTIONS`） | **403** | `http.rs:176` → `:101` |
  | 面板 origin + `OPTIONS` | **204** 预检（精确 ACAO + 允许方法/头） | `http.rs:179-185` → `:196` |
  | 非面板 / 无 `Origin` + `OPTIONS` | **405** | `http.rs:179-185` → `:259` |
  | 无 `Origin`（插件 Node `fetch`）+ 无/错令牌 | **401** | `http.rs:186-188` → `:248` |
  | 面板 origin + 非 `OPTIONS` + 无/错令牌 | **401**（带精确 ACAO） | `http.rs:186-188` → `:103-104` |
  | 无 `Origin` 或面板 origin + 正确令牌 | 进入 `App::handle`（面板 origin 的响应带 ACAO） | `http.rs:189` 之后 / `:140` |

   注一：`/api/page-event/extra` **不是**豁免路径（精确匹配，非前缀）——带非面板 `Origin` 走 403，无 `Origin` 无令牌才落 401。
   注二：401 与 403 **共用同一个响应体** `auth_error`（`http.rs:248`，`{success:false,error:"unauthorized"}`，不回显期望令牌）；区分只体现在状态码上。TS 侧把 401/403 一并归入 `HttpOutcome.auth`（`client.ts:681-682`）。
- **唯一豁免：精确路径 `/api/page-event`**，且该豁免**最先判定、先于 `Origin`**。页面注入脚本运行在远程页面源下、拿不到令牌，浏览器又必带 `Origin`，故该路径跳过 `Origin` 与令牌两项校验；但 `OPTIONS` 不豁免（非面板 origin 仍 405）。残余风险：该端点只把 url/title/icon 写进 `TabState` 提示、不执行操作、无敏感回读，被同机页面伪造最多导致地址栏/标题错乱；若未来给它加敏感字段或副作用，必须重新评估豁免。
- **面板 origin 放行的边界**：外部网页无法伪造 `Origin`（浏览器强制写入），`null`（沙箱 iframe / `file://`）、带端口、子域变体都不在白名单，故放行不扩大对"其它站点"的攻击面；**令牌仍是最终闸门**（面板 origin 同样必须有正确 `x-bt-token`），Origin 只做纵深防御。残余风险：同机进程可自带任意 `Origin` 构造请求（绕过 Origin 语义），但同机进程本就能读到实例令牌，故非新增风险。
- 校验顺序：`page-event` 精确判定 → `Origin`（豁免路径跳过）→ `OPTIONS` → 令牌（豁免路径跳过）→ 读 body → `App::handle`（`http.rs:70` 头注释同步）。

### TS 侧（`src/client.ts`）

- `ShellInstance.token`(`client.ts:112`，实例创建期确定、随实例不可变)。**1.10.1 起不再有 `authHeaders()` 辅助函数**(已删除)：`x-bt-token` 由唯一 HTTP 原语 `httpCall` **内联设置**(`client.ts:719` 的 `headers:{"Content-Type":…, "x-bt-token":token}`)，所有调用方(`postJson` `:814`、`probeOnce` `:665`、`waitForReady` `:239`、`registerNotifyUrl` `:497`、`closeSession` 的 `/api/close` `:583`)都把令牌作为参数交给 `httpCall`，不存在第二处拼头代码。
- **启动模式**：`spawnForSession`(`client.ts:327`)生成 `randomBytes(16).toString("hex")`(`client.ts:332`)，以 `--token` 传给 shell；端口重试沿用同一令牌。
- **附着模式**：令牌取 `process.env.BT_SHELL_TOKEN`(`requireAttachToken`，`client.ts:95`)，为空直接抛 `client.attach_need_token`(i18n zh/en 同键)，不去连注定 401 的端点；令牌错误(401/403 → `HttpOutcome.auth`)是**确定性失败**，`waitForReady(..., authHint=true)` 立即抛 `client.attach_bad_token`，不等 20s 就绪超时。外部 shell 必须用同一令牌启动(`--token` 或同值环境变量)。
- 纯查询 `probeStatus`(`client.ts:642`)按实例探测(每个实例带自己的令牌，attach 实例用环境变量值)，`get_browser_status` 仍是纯查询、不 spawn。

### 注意 / 已知边界

- **页面 webview 的 `/api/page-event` 上报通道**（`rust/src/ui/mod.rs` 注入脚本，跨源 beacon 必带 `Origin` 且无法持有令牌）：服务端按**精确路径**豁免校验后可用（见上），SPA 路由/标题/图标的即时上报不退化。注意：`rust/src/ui/mod.rs:215`、`rust/src/service.rs:682` 的注释仍写「CORS 已移除、OPTIONS 一律 405」，那是 1.10.0 口径，**1.11.0 起已过时**（面板 origin 恢复最小 CORS；非面板来源的 CORS/OPTIONS 语义未变）——Rust 注释尚未同步，以本文与 `http.rs:1-9` 头注释为准。
- **面板 Webview 的 `/api/*` 调用**：通过 Tauri 命令 `api_token` 读取本实例令牌（`rust/src/main.rs` 的 `ApiToken` 受管状态，与 `http::serve` 同源），前端启动后首次 `api()` 时缓存并带 `x-bt-token`；读取失败只打面板控制台错误、不阻断面板，令牌不写入 DOM/HTML/日志。**面板 origin（`http(s)://tauri.localhost` / `tauri://localhost`）自 1.11.0 起在白名单内**：预检走 204 + 精确 ACAO，实际请求照常校验令牌；外部站点仍 403。
- 令牌是**单实例共享秘密**，不做多用户/多会话区分；附着模式下所有 sid 共用同一个外部 shell 与同一令牌。

## 1.10.1 HTTP 调用统一原语与文本脱敏（插件侧，零 Rust 改动）

### `httpCall`：唯一出口与五类 outcome

`httpCall(baseUrl, token, path, payload, opts?)`（`src/client.ts:704`）是所有对 shell 的 POST 的**唯一原语**：统一设置 `Content-Type` + `x-bt-token`(`:719`)、统一 `AbortController` 超时(`:711-713`，省略 `timeoutMs` 则不限)。**从不抛错**，只返回 `HttpOutcome`(`:676-686`)：

| kind | 含义 | 判定顺序依据 |
|---|---|---|
| `ok` | 传输成功且服务端 `success:true` | 最后看 `success` 字段 |
| `server` | 服务端明确 `success:false`（业务错误，**实例仍存活**） | 注释 `:679-680` |
| `auth` | HTTP 401/403（**确定性鉴权失败**，不是连接故障） | `:681-682` |
| `transport` | fetch 本身抛错或请求超时（端口没人听 / 进程已死），包成 `ConnectionError` | `:683-684` |
| `badjson` | 拿到 HTTP 响应但响应体不是 JSON（协议异常，**不等于实例死亡**） | `:685-686` |

归类顺序：fetch 抛错/超时 → `transport`；401/403 → `auth`；响应体非 JSON → `badjson`；其余看 `success`（注释 `:693-694`）。**语义由调用方决定，不在原语里做**（注释 `:696-702`）：

| 调用方 | 策略 |
|---|---|
| `postJson`(`:814`) 工具调用 | ok→data；server→抛错（**经 `sanitizeForModel`**）；auth→抛 `client.auth_rejected`/`client.attach_bad_token`；transport→抛 `ConnectionError`（供 `callApi` 判死）；badjson→抛 `client.bad_response`（**不判死**） |
| `probeOnce`(`:665`) 探活 | 只有 ok 有值，其余 `null` |
| `probeStatus`(`:642`) | 逐实例短超时探测，不 spawn |
| `registerNotifyUrl`(`:497`) 增强路径 | 只有 ok 返回 `true`，其余 `false` |
| `waitForReady`(`:239`) 就绪轮询 | ok 结束；transport/server/badjson 继续等；attach 模式下 auth **立即报错**（`authHint`） |
| `closeSession`(`:563`) 关闭 | 忽略任何结果 |

### `ConnectionError` / `isConnectionError`

- `ConnectionError`（`client.ts:759`）：由 `httpCall` 在**抛错点显式标记**的传输层错误类型，保留 `cause` 供取证（`:760-771`）。动机（注释 `:754-757`）：同一份插件代码会跑在 **Bun**（opencode 默认运行时）上，Bun 的 fetch 失败是 `TypeError: Unable to connect. Is the computer able to access the url?` 且**没有 `cause.code`**；Node/undici 是 `fetch failed` + `cause.code=ECONNREFUSED`。只靠猜文案/码跨运行时必漏（1.10.1 真机缺陷），因此判断方**首认类型**。
- `isConnectionError`（`client.ts:782`）三级判定：① `instanceof ConnectionError`（运行时无关）② `cause.code`/`code` 命中 `ECONNREFUSED|ECONNRESET|…|ConnectionRefused`（`:788-790`）③ 文案兜底 `Unable to connect` / `fetch failed` / `Failed to connect` 等（`:792-796`）。HTTP 4xx/5xx **不算**连接错误（`http.rs` 的 401/403/405 是正常响应，归 `auth`/`server`）。导出仅供自验/单测直接构造运行时文案验证。

### `sanitizeForModel`（模型可见文本脱敏）

`src/util/sanitize.ts:60`，默认上限 `DEFAULT_MAX_LEN=300`（`:21`）。规则：先把 URL 用占位符保护（`URL_RE` `:27`），再掩码 Windows 盘符路径、UNC、POSIX 绝对路径；**`/api/*`、`/notify/*` 接口路径与 URL 原样保留**（`:44`、`:78`、还原 `:81-82`），末尾按长度截断（`:85`）。幂等。调用点：`postJson` 的服务端错误文本（`client.ts:821`）、工具层参数错误/读文件失败/命令失败三处（`index.ts:430,469,480`）、二进制下载原因文本（`binary.ts:193,250`）。

### 参数默认值口径

- `--timeout`：合法区间 **[100, 60000] ms**（插件 `src/index.ts:90-91` 与 Rust `service.rs:96-97` 同口径）；非正整数直接抛 `cli.err.timeout_invalid`，超界**钳制并回显**（插件 `timeoutNote` `index.ts:99-108`、Rust 二次钳制 `service.rs:113`）。未传时用服务端默认 **15s**（`ui/mod.rs:35 EVAL_TIMEOUT_MS`、`control/mod.rs:43 DEFAULT_NAV_TIMEOUT_MS`）。
- 可访问性树：输出层默认展示深度 `maxDepth ?? 8`（`src/index.ts:204`，客户端裁剪参数）；**服务端遍历上限是另一回事**——Rust 侧 `AX_MAX_DEPTH=64`、`AX_MAX_NODES=5000`（`rust/src/control/accessibility.rs:355,359`，与 Linux `DOM_AX_JS` 同口径），触顶只截断并留日志、不 panic。两者别混。
## 1.10.0 其他行为（队列 / 记录 / 标签 / DevTools / Linux 截图）

### 待发送队列

- 面板「发送」把本次新标记为 `sent` 的记录**追加**进 shell 侧队列（`append_sent_records`，`ui/mod.rs:1450`）：按记录 `index` 去重（同一批重复 send 不会重复入队），上限 `SENT_QUEUE_MAX=500`，超出丢最旧。旧实现是覆盖写——插件上一批还没 drain 完就再点发送，未消费的记录会被整批丢掉。
- 队列由**插件** drain 消费（通知 / 服务就绪各触发一次，幂等）；shell 只负责追加与通知，见上方投递小节。

### 批注 / 截图记录

- 记录表上限 `MAX_RECORDS=200`（`annotate.rs:395`），超出丢最旧；`index` 单调递增（不按 `len` 推算，避免丢旧记录后与队列里的旧 index 重号）。
- `/api/annotate/records` 的 `includeImages` **默认 false**(`service.rs:1279` 读取)：每条记录只回 `hasImage:boolean`(`service.rs:1294`)，**不带 PNG base64**；只有显式传 `includeImages:true` 时该条才附 `image`(`service.rs:1296-1300`)，避免整批大图塞进响应。取用方式与上文「发送修复」条一致：`list_records`(`index.ts:205`)不传，只看 `hasImage`；`read_record_content`(`index.ts:206`)传 `includeImages:true` 取回 base64 并落盘到该会话临时目录。
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

## 待核实（本轮无法一手取证的项）

以下两条**不是本文结论**，只登记「无法核实」，不做断言：

1. **`librsvg2-dev` 是否为 Linux 构建/运行必需**：`Cargo.lock` 无对应 crate，`linux-verification.md:17` 的安装清单里带着它，但去掉能否构建/是否有 SVG 渲染差异，本机未实测（`linux-support.md:74` 同样标注「待核实」）。需要一次「不装 librsvg2-dev 的干净构建 + SVG 页面渲染」实测才能定。
2. **`show_notification --type` 的合法取值集合**：Rust `service.rs:273-291` 只读 `body["type"]`、缺省 `"ok"`（`:275`），随后**原样注入** overlay 的 `notify(m,t)`（`:281`）；插件层（`index.ts:176`）也不校验。已知实际用到的值有 `"err"`（drain 失败路径 `index.ts:708`）。**完整取值枚举与未知值的渲染降级行为**要看覆盖层前端 `notify` 实现（未逐一核实），故不在本文给出「支持哪些 type」的清单。

（对比：`console_logs --type` 是**另一套**取值 —— `all/error/warning/log/info/debug`，服务端校验并报错 `service.rs:794-811`，与通知类型无关，勿混。）

