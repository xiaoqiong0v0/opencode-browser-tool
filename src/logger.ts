// 共享日志模块:全局单例 logger,避免各文件重复创建(与 opencode-ssh-tool 的 src/log.ts 同构)
// 落盘约定由 @xiaoqiong0v0/opencode-plugin-logger 决定:
//   ~/.opencode/plugins-log/<YYYYMMDD>.log,行格式 `[时间] [级别] browser-tool <内容>`
//   需在 ~/.config/opencode/plugin-logger.jsonc 里 `enabled: true` 才会写文件(默认 false)
import createLogger from "@xiaoqiong0v0/opencode-plugin-logger";

export const log = createLogger("browser-tool");
