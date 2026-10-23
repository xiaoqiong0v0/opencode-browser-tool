# docs 索引

> 本目录放**过程与方案**类文档：架构决策与历史、平台适配口径、并发契约、复验清单。
> **对外行为与命令清单以项目根 `README.md` 为准**，本目录不重复维护；看当前实现状态先读 `implementation.md`。

共 5 篇（含本索引另计），链接均已核对可打开：

- [implementation.md](./implementation.md) — 当前实施状态与关键经验（含可信输入、二进制分发/版本策略）
- [linux-support.md](./linux-support.md) — Linux 支持说明：布局垫片/缩放/透明/截图与硬性约定（禁忌：WEBKIT 环境变量、CSD 装饰、emoji 图标）；§1 能力矩阵、§7「平台能力与降级策略」给命令级可信/降级矩阵
- [linux-verification.md](./linux-verification.md) — Kali/Linux 可信输入复验清单与预期结果（一键脚本 `tests/manual/verify-linux.sh`；§3.1 为 1.10.0 起的令牌前置）
- [ASYNC.md](./ASYNC.md) — 异步/并发交互点清单（共享状态、保护手段、顺序契约、位置；**新增异步/并发必须同批登记，改动同步更新**）
- [v5-tauri-architecture.md](./v5-tauri-architecture.md) — **历史设计（v5）**：立项时的方案变更说明、平台能力对照与里程碑；**现状以根 `README.md` / `implementation.md` / `linux-support.md` / `ASYNC.md` 为准**，文内顶部已列与现状不一致处（macOS 未实现、发送链路改事件驱动、鉴权令牌、工具/子命令数）
