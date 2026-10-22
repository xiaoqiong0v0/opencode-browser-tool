# docs 索引

- [v5-tauri-architecture.md](./v5-tauri-architecture.md) — 当前架构：V5 跨平台 Tauri 多 Webview 方案（原始架构文档，保留不改）
- [implementation.md](./implementation.md) — 实施状态与关键经验（含可信输入、二进制分发/版本策略）
- [linux-support.md](./linux-support.md) — Linux 支持说明：布局垫片/缩放/透明/截图与硬性约定（禁忌：WEBKIT 环境变量、CSD 装饰、emoji 图标）；§7「平台能力与降级策略」给命令级可信/降级矩阵
- [linux-verification.md](./linux-verification.md) — Kali/Linux 可信输入复验清单与预期结果（一键脚本 `tests/manual/verify-linux.sh`）
- [ASYNC.md](./ASYNC.md) — 异步/并发交互点清单（共享状态、保护手段、顺序契约、位置；新增并发必须登记）
