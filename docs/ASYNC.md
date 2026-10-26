# 异步 / 并发交互点清单

> 规则：新增异步或并发路径必须在本表登记；改动同步更新。只列**跨调用共享的可变状态**与顺序契约。
> 位置写「文件:函数」（不写死行号）：插件侧在 `src/`，shell 侧在 `rust/src/`；跨语言条目把两侧来源都列出。

| 交互点 | 共享状态 | 保护手段 | 顺序契约 | 位置 |
| --- | --- | --- | --- | --- |
| 同会话并发启动 | `startingSid: Map<sid, Promise>` | 同一 sid 复用同一个启动 Promise；`finally` 删除 | 第二个调用 await 第一个；启动完成才 `sessions.set` | `client.ts:ensureInstance` |
| 启动 vs 关闭 | `closedSids: Set<sid>`、`startingSid` | `closeSession` 先置 `closedSids`；启动链在 `spawnForSession` 后自查并自毁刚 spawn 的子进程 | 关闭意图优先：启动完成若已置位则不注册实例 | `client.ts:ensureInstance` / `closeSession` |
| 显式打开（open_window） | `closedSids` | `openWindow` 入口清一次；`ensureInstance(allowClosed)` spawn 成功 `sessions.set` 后再清一次；`await` 返回后再清一次 | 成功打开后「已关闭」标记必须消失，避免并发 callApi 探活失败重新置位导致误报已关闭 | `client.ts:openWindow` / `ensureInstance` |
| 连接层错误标记死亡 | `sessions`、`attachInstance`、`closedSids` | 仅当注册项仍指向该实例才删除（防止误删重开的新实例）；失败路径置 `closedSids`，**绝不自动重开** | 标记死亡与 `open_window` 并发时，以 open_window 成功路径的清理为准 | `client.ts:callApi` / `markInstanceDead` |
| 附着模式 notify 注册 | `attachNotifyRegistered: Set<key>`、`attachNotifyInFlight: Map<key, Promise<boolean>>` | 同 key 并发共享同一个 Promise；settle 后从 in-flight 清理；失败不写「已注册」，下次调用重试 | 成功才进入「已注册」集合；会话删除 / 断开时同时清两个集合 | `client.ts:ensureAttachNotify` / `clearAttachNotify` |
| spawn 后 notify 注册 | shell 侧 `UiState.notify_url`（`Mutex<Option<String>>`，经 `/api/notify-url` 覆写） | 注册失败 300ms 后重试一次，仍失败只记 error（不阻断工具调用成功） | 注册在实例 `waitForReady` 就绪之后、`sessions.set` 前后均可；失败不影响实例可用性 | `client.ts:registerNotifyUrl` / `spawnForSession` → `service.rs:set_notify_url`（状态在 `ui/mod.rs:UiState.notify_url`） |
| 会话删除回收数据目录 | 文件系统 `sessions/<profile>/<sid>` | sid 形态校验 + `isInsideDir` containment；`rmSync` try/catch；失败 800ms 后重试一次（定时器 `unref`） | 先 `closeSession` + `clearClosedState` 再删目录；只删该 sid 目录，绝不删 sessions 根或其它 sid | `index.ts:removeSessionDataDir` / `session.deleted` |
| 会话删除与 attach notify | `attachNotifyRegistered` / `attachNotifyInFlight` | 按 sid 后缀清理（`baseUrl\|sid`） | 删除事件只清理该 sid 的键，不影响其它会话 | `client.ts:clearAttachNotifyForSid` |
| 通知 drain | `drainingSid: Set<sid>` | 同 sid 重入直接返回；`finally` 清理 | 每 sid 串行 drain；不因 drain 触发 spawn | `index.ts:drainAnnotate` |
| 插件通知监听（单例 + 端口绑定） | 模块级 `server: Server\|null`、`port: number`、`onNotify: 回调` | `startNotifyServer` 幂等短路（`server && port` 时直接复用已监听端口，不重建）；只绑 `127.0.0.1` + `listen(0)` 取随机空闲端口；`srv.unref()` 不拖住插件进程退出；`listen` 报错走 `reject` | 每次调用先把 `onNotify` 覆写为最新 handler（插件重载后仍是新回调）；sid 由 URL 路径 `/notify/<sid>` 解出，非法百分号编码按空 sid 处理（异常不许冒出 request listener）；请求体超 4096B 直接 `destroy`（通知只传最小信息）；**先写完 200 响应再回调 drain**，回调异常吞掉不影响响应 | `notify-server.ts:startNotifyServer` / `getNotifyUrlForSession`；调用点 `index.ts:setupAnnotateDelivery` |
| 二进制下载并发（同进程 / 跨进程） | 模块级 `inflight: Promise\|null`、`current: BinaryStatus`；文件系统 `<target>.lock`、`<target>.part`、`<target>`（可执行文件）、版本旁文件 `bt-shell.version.json` | 同进程：`startDownload` 复用同一 `inflight` Promise，`finally` 置空；跨进程：`writeFileSync(flag:"wx")` 原子建锁，`EEXIST` 且 mtime 未超 `LOCK_STALE_MS`（10 分钟）→ 不下载，改为 `waitForValidFile` 轮询等待（上限 60s）；超期视为陈旧锁 → 删除后重取；非 `EEXIST` 错误不阻塞下载 | 顺序固定：拉 SHA256SUMS → 旧布局目录迁移 → **幂等检查（目标存在且 sha 匹配则跳过，不抢锁）** → 抢锁后才下载 → 写 `.part` → sha256 校验（不符删 `.part`）→ `renameSync` 原子替换 → 非 Windows `chmod 755` → 写旁文件 → `finally` 释放锁；旁文件只在最终文件合法后写（sha/asset/tag 全同则不改写）；`ensureBinary` 有界等待，超时返回当前状态，不无限等 | `binary.ts:startDownload` / `doDownload` / `acquireLock` / `releaseLock` / `waitForValidFile` / `ensureSidecar` / `migrateLegacyDirs` |
| API 鉴权令牌 | `ShellInstance.token`（每实例一份）、`BT_SHELL_TOKEN`（attach 模式）、Rust `HttpState.token`、面板前端 `apiToken` 缓存 | 令牌在**实例创建期**确定、随实例不可变（Rust 侧只读 `Arc<str>`）；attach 缺失环境变量直接抛错；前端并发 `api()` 共享同一个在途 `invoke` Promise；**面板 origin 白名单 + 最小 CORS**（`PANEL_ORIGINS` 精确匹配，预检 204 + 精确回显 ACAO，绝不 `*`）只作用于 `Origin` 头，不放宽令牌 | spawn 生成令牌并随 `--token` 传给子进程，端口重试沿用同一令牌；attach 令牌只来自环境变量，所有 `/api/*` 请求（含 probe/notify/close）带 `x-bt-token`；**面板经 Tauri `invoke("api_token")` 读取同一令牌**；**`/api/page-event` 精确路径豁免 `Origin`+令牌**（页面脚本无法持有令牌）；**服务端判定顺序固定：`page-event` 豁免 → `Origin` → `OPTIONS` → 令牌**，面板预检（`OPTIONS`）**无状态、不校验令牌**、与实际请求互不共享可变状态（实际请求仍必须带令牌，401 响应也带 ACAO 供前端读到错误体） | `client.ts:spawnForSession` / `attachToService` / `http.rs:handler` / `http.rs:decide_auth` / `main.rs:api_token` / `frontend/src/index.ts:getApiToken` |
| 插件通知投递 | `NOTIFY_TX: OnceLock<mpsc::Sender<(String, usize)>>` | 单线程 FIFO worker：`mpsc` 队列 + 独立线程 `current_thread` tokio runtime；一条通知**首发 + 3 次重试**（300/600/1200ms 退避，单次超时 2s）后才处理下一条 | 多次通知严格按入队顺序投递（**保序**、不并发），不阻塞 UI/事件线程；失败只记日志并继续后续通知 | `ui/mod.rs:notify_plugin` / `notify_worker` / `post_notify` |
| 待发送队列 | `state.sent_records`（shell 内全局唯一，attach 模式下跨 sid 共享） | 追加 + 按记录 `index` 去重 + 上限 `SENT_QUEUE_MAX=500`（超出丢最旧）；`Mutex` 保护 | 只追加不覆盖（旧实现覆盖会丢未 drain 的批次）；同一批重复 send 不重复入队；消费由**插件 drain（通知触发）**完成，队列自身不并发消费 | `ui/mod.rs:append_sent_records` / `send_all_records` |
| 批注/截图记录上限 | `state.records` | `MAX_RECORDS=200`（超出丢最旧）；`index` 单调递增（不按 `len` 推算，避免重号） | 发送只标 `sent`、不清表（面板按未发送过滤）；旧记录可被新记录按上限挤出 | `annotate.rs:push_record_capped` / `Annotator::push_record` |

## 已知边角（有意保留）

- `startNotifyServer` 的幂等靠 `server && port` 置位，而置位发生在 `listen` 回调里：**settle 之前的并发调用会各建一个监听**（各自占一个随机端口）。现状调用点唯一（插件加载时 `setupAnnotateDelivery` 调一次），故未加 in-flight 去重；若将来出现多处调用，需按 binary 下载的做法共享同一个 Promise。
- `acquireLock` 对「非 `EEXIST` 的建锁错误」直接返回 `true`（视为抢到，继续自己下载）：锁只是去重优化，不能因锁本身失败而让下载不可用；代价是这种场景下可能两个进程各下一次，最终仍由 `.part`→`rename` 保证文件合法。
- `getActiveProfile()` 读取到非法值时回落到 `"default"`（启动路径不允许抛错），仅记录一次日志；写入口（`profile --set`）已做名字校验。
- `@file:` 只允许全局 tmp 或当前会话 tmp 之内的文件；判定用 `path.relative` containment，不做符号链接解析（调用方只能传路径字符串）。
