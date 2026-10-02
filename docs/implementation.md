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
- [x] 纯查询命令不懒启动：`get_browser_status` 走 `probeStatus()`（短超时 HTTP 探测，**不 spawn**）；未运行时返回未运行 + bt-shell 二进制状态
- [x] 发送（批注/截图）修复（以旧 `opencode-playwright-tool` 为参考）：面板"发送"**只发文本**（与参考一致；`session.prompt` 的 parts 不支持图片 —— `image/*` 走 `image.normalize` 返回 400，参考实现从不这么发）；截图图片走**工具结果附件**交付：`attachments:[{type:"file", mime:"image/png", url:"data:image/png;base64,…"}]`（关键：opencode 只接受 `url` 以 `data:` 开头且含 `,` 的附件，旧形状 `url:""+data` 会被静默丢弃；见 `message-v2.ts:170-187`，该路径**绕过** `image.normalize`）；发送时截图另落盘并在文本里给出路径；显式检查 SDK 返回的 `{error}`（不抛异常）并 log+notify；轮询器出错不再 `clearInterval`；发送后**面板列表清空**（仅面板列表为空、不再显示已发送项——与参考实现 `clearRecords()` 语义一致；Rust 侧 `panel_records`/`records-changed` 均按 `!sent` 过滤），**记录本身保留并标记 `sent`**（Rust `records` 不清空；`list_records`/`read_record_content` 走 `/api/annotate/records` 仍能读到全部，含截图）

- [x] 投递前 session id 可靠来源：真实 GUI 取证 `client.session.list()` 返回 `sessions=0` 导致无法投递；改为**优先用插件钩子缓存的 sid**（`Event.properties.sessionID`、`session.created/updated/deleted` 的 `properties.info.id`、`chat.message`/`tool.execute.before`/`shell.env` 入参 `sessionID`、工具 `context.sessionID`），`list()` 仅兜底并打印原始包络；仍拿不到则 log+notify 不静默
- [x] 日志规范（插件侧）：统一用公共包 `@xiaoqiong0v0/opencode-plugin-logger`，项目内 `src/logger.ts` 导出共用实例 `log`；**禁止用 `console.*` 往 stdout/stderr 打**（会污染 opencode TUI），`src/**` 的 `console.*` 仅允许出现在独立 CLI 的 `isMain` 守卫内（`src/binary.ts`）；Rust 的 `println!`/`eprintln!` 默认被 spawn 管道吞掉，必须由插件把子进程 stdout/stderr 转发进 logger 才可见（`src/client.ts` startService）

- [x] 面板发送改为**推送式投递**（对齐参考 `opencode-playwright-tool` 的 bridge 机制）：插件本地监听 `127.0.0.1:<随机端口>/notify`（仅接收最小 `{count}`，大内容仍走 drain）；shell `send_all_records` 标记 sent 后 **fire-and-forget** POST 一次（2s 超时、最多重试 2 次；失败 `eprintln!` 经插件转发可见）；地址传递：spawn 传 `--notify-url` / 环境变量 `BT_SHELL_NOTIFY_URL` / 附着模式用 `/api/notify-url` 注册。原 `startAnnotatePoller`（`setInterval` 2s）**已删除**，改为事件驱动 `drainAnnotate`：通知即 drain + 服务就绪/插件启动各 drain 一次（覆盖窗口，幂等）；拿不到 sid 时仍 log+notify

### 关键经验
- **投递批注取 session id**：`client.session.list()` 是 GET，SDK 会追加 `?directory=<插件客户端目录>`（`@opencode-ai/sdk/dist/client.js` 的 `rewrite`），在真实插件进程里实测可能返回空包络（本项目一次真实 GUI 操作为 `sessions=0`），因此**不可作为唯一来源**；可靠来源是插件钩子：`chat.message`/`tool.execute.before`/`shell.env` 入参 `sessionID`、工具 `context.sessionID`、以及 `Event.properties.sessionID`——注意 `session.created/updated/deleted` 的 id 在 `properties.info.id`（不是 `properties.sessionID`），旧代码读 `event.data.sessionID` 是错的。实现见 `src/session-id.ts`
- **日志降噪**：`/api/annotate/consume-sent`（`rust/src/service.rs`）仅在**非空**时 `eprintln!`；投递已是推送式（`/notify` 到达即 drain），不存在周期性空拉取；插件 drain 也只在非空/出错时记录
- `eval_with_callback` 自动 JSON 序列化 JS 返回值，表达式直接返回对象（勿双重 stringify）
- **opencode 宿主对 tool 返回值要求字符串**：命令 `run` 返回对象会让宿主（Bun/JSC）报 `undefined is not an object (evaluating 'c.split')`；结构化结果请 `JSON.stringify` 或自行格式化（截图类返回 image attachment 属例外）
- **Windows 可访问性树（CDP）**：`Accessibility.getPartialAXTree` 即使 `fetchRelatives=false` 也只返回命中节点自身（其 `parentId` 指向不在结果内的祖先）→ 取"某选择器子树"应改用 `DOM.getDocument`+`DOM.querySelector`+`DOM.describeNode`(取 backendNodeId) 定位命中 AX 节点，再用 `Accessibility.getFullAXTree` 并以该节点为根建树；CDP 会把文本拆成逐字符 `StaticText` + `InlineTextBox`，且 `html`/`body`/无名容器为 ignored → 需丢弃 InlineTextBox、把 StaticText 文本合并进父节点 name、并按"name 值为空"跳过无名 generic（原判据 `name` 字段缺失对 CDP 恒 false）
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

## 待办

- macOS：截图（WKWebView snapshot）、可信输入（无 CDP/GDK 通道，现走降级或明确报错）等平台分支
- Linux：`upload_file` / `drag` 的可信通道未确证（当前**明确报错**，不假成功）；可访问性树为 DOM 近似
- 真实 Linux 桌面（GNOME/KDE）复验（WSLg 已通过 15 项，见 `linux-verification.md`）
