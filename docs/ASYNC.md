# 异步 / 并发交互点清单

> 规则：新增异步或并发路径必须在本表登记；改动同步更新。只列**跨调用共享的可变状态**与顺序契约。
> 位置以 `src/` 内文件:函数 为准（不写死行号）。

| 交互点 | 共享状态 | 保护手段 | 顺序契约 | 位置 |
| --- | --- | --- | --- | --- |
| 同会话并发启动 | `startingSid: Map<sid, Promise>` | 同一 sid 复用同一个启动 Promise；`finally` 删除 | 第二个调用 await 第一个；启动完成才 `sessions.set` | `client.ts:ensureInstance` |
| 启动 vs 关闭 | `closedSids: Set<sid>`、`startingSid` | `closeSession` 先置 `closedSids`；启动链在 `spawnForSession` 后自查并自毁刚 spawn 的子进程 | 关闭意图优先：启动完成若已置位则不注册实例 | `client.ts:ensureInstance` / `closeSession` |
| 显式打开（open_window） | `closedSids` | `openWindow` 入口清一次；`ensureInstance(allowClosed)` spawn 成功 `sessions.set` 后再清一次；`await` 返回后再清一次 | 成功打开后「已关闭」标记必须消失，避免并发 callApi 探活失败重新置位导致误报已关闭 | `client.ts:openWindow` / `ensureInstance` |
| 连接层错误标记死亡 | `sessions`、`attachInstance`、`closedSids` | 仅当注册项仍指向该实例才删除（防止误删重开的新实例）；失败路径置 `closedSids`，**绝不自动重开** | 标记死亡与 `open_window` 并发时，以 open_window 成功路径的清理为准 | `client.ts:callApi` / `markInstanceDead` |
| 附着模式 notify 注册 | `attachNotifyRegistered: Set<key>`、`attachNotifyInFlight: Map<key, Promise<boolean>>` | 同 key 并发共享同一个 Promise；settle 后从 in-flight 清理；失败不写「已注册」，下次调用重试 | 成功才进入「已注册」集合；会话删除 / 断开时同时清两个集合 | `client.ts:ensureAttachNotify` / `clearAttachNotify` |
| spawn 后 notify 注册 | — | 失败 300ms 后重试一次，仍失败只记 error（不阻断工具调用成功） | 注册在实例就绪后、`sessions.set` 前后均可，失败不影响实例可用性 | `client.ts:spawnForSession` |
| 会话删除回收数据目录 | 文件系统 `sessions/<profile>/<sid>` | sid 形态校验 + `isInsideDir` containment；`rmSync` try/catch；失败 800ms 后重试一次（定时器 `unref`） | 先 `closeSession` + `clearClosedState` 再删目录；只删该 sid 目录，绝不删 sessions 根或其它 sid | `index.ts:removeSessionDataDir` / `session.deleted` |
| 会话删除与 attach notify | `attachNotifyRegistered` / `attachNotifyInFlight` | 按 sid 后缀清理（`baseUrl|sid`） | 删除事件只清理该 sid 的键，不影响其它会话 | `client.ts:clearAttachNotifyForSid` |
| 通知 drain | `drainingSid: Set<sid>` | 同 sid 重入直接返回；`finally` 清理 | 每 sid 串行 drain；不因 drain 触发 spawn | `index.ts:drainAnnotate` |

## 已知边角（有意保留）

- `getActiveProfile()` 读取到非法值时回落到 `"default"`（启动路径不允许抛错），仅记录一次日志；写入口（`profile --set`）已做名字校验。
- `@file:` 只允许全局 tmp 或当前会话 tmp 之内的文件；判定用 `path.relative` containment，不做符号链接解析（调用方只能传路径字符串）。
