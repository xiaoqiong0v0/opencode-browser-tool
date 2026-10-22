/**
 * 模型可见文本的最后一道闸：把本机绝对路径掩码成 `<path>`。
 *
 * - 只用于**即将返回给模型**的文本（工具返回值、状态文案、抛给模型的错误 message）。
 * - 插件日志（logger / stdout / stderr）不走本函数，原始路径仍完整保留在日志里，便于取证。
 * - 这里是兜底而非唯一防线：各出口应尽量只输出「环境变量名 / 配置项名」，本函数负责拦住
 *   底层 fs/fetch 错误里意外夹带的路径。
 *
 * 为什么不用正则去猜错误文案：
 * 错误分类（鉴权 / 连接 / 服务端 / 响应非 JSON）由 `client.ts` 的 HTTP 原语按状态码与
 * 抛错点**显式标记**（`HttpOutcome` / `ConnectionError`），不依赖 message 措辞；本函数只做
 * 已决定要展示的文本的路径脱敏，不推断错误语义、不改变调用方的分支判断。两者分工明确：
 * 分类在 HTTP 层，脱敏在展示层。若在这里用正则猜「这是不是路径/是不是某个错误」，跨运行
 * 时（Node/Bun）文案差异必然漏判，反而更难维护。
 */

/** 本机绝对路径的统一占位符 */
const PATH_PLACEHOLDER = "<path>";

/** 默认截断长度（字符数，含省略号） */
const DEFAULT_MAX_LEN = 300;

/** 截断标记 */
const ELLIPSIS = "…";

/** URL 先用占位符保护，避免后面的路径规则误伤 `https://host/a/b` 里的 `/a/b` */
const URL_RE = /\b(?:https?|wss?|ftp):\/\/[A-Za-z0-9\-._~:/?#[\]@!$&()*+,;=%]+/gi;

/** 引号内的绝对路径（允许含空格）：只替换内容，保留引号 */
const QUOTED_PATH_RE = /(["'])((?:[A-Za-z]:[\\/]|\\\\|\/)[^"'\r\n]+)\1/g;

/** Windows UNC（反斜杠形式，如 `\\server\share\x`；也覆盖 `\\?\C:\...`） */
const UNC_RE = /\\\\[^\s'"<>|，；、）\]]+/g;

/** Windows UNC 的正斜杠形式（`//server/share/x`）；URL 已提前保护 */
const UNC_FWD_RE = /(?<![\w:/])\/\/[A-Za-z0-9._$-]+\/[^\s'"<>|，；、）\]]+/g;

/** Windows 盘符绝对路径（`C:\x` / `C:/x`）；负向前瞻排除 URL scheme 里的 `s:/` */
const WIN_ABS_RE = /(?<![\w:/])[A-Za-z]:[\\/][^\s'"<>|，；、）\]]*/g;

/** POSIX 绝对路径（至少两段，如 `/home/u/x`、`/tmp/a/b`、`/etc/passwd`）；不匹配相对片段 `a/b` */
const POSIX_ABS_RE = /(?<![\w.:/~-])\/(?:[A-Za-z0-9._~+@%-]+\/)+[A-Za-z0-9._~+@%-]*/g;

/** 接口路径前缀（`/api/...`、`/notify/...`）：属于诊断信息而非本机路径，POSIX 匹配命中后原样保留 */
const KEEP_API_PATH_RE = /^\/(?:api|notify)\//;

/** URL 保护占位符（控制字符不会出现在正常文案里） */
const URL_TOKEN_RE = /\u0001(\d+)\u0002/g;

/**
 * 把文本中的本机绝对路径掩码为 `<path>`，并按需截断。
 *
 * 覆盖 Windows 盘符路径、UNC、POSIX 绝对路径；URL、版本号、相对路径（`a/b`、`dist/index.js`）、
 * 以 `/api/`、`/notify/` 开头的接口路径与已存在的 `<path>` 保持不变。幂等：对同一文本重复调用结果稳定。
 *
 * @param text 模型可见文本
 * @param maxLen 截断上限（字符数，含省略号），默认 300
 * @returns 脱敏（并按需截断）后的文本
 */
export function sanitizeForModel(text: string, maxLen: number = DEFAULT_MAX_LEN): string {
  if (!text) return text;

  // 1) 先保护 URL（恢复时原样放回）
  const urls: string[] = [];
  let out = text.replace(URL_RE, (m) => {
    const token = `\u0001${urls.length}\u0002`;
    urls.push(m);
    return token;
  });

  // 2) 引号内路径（可能含空格）→ 保留引号，只换内容
  out = out.replace(QUOTED_PATH_RE, (_m, quote: string) => `${quote}${PATH_PLACEHOLDER}${quote}`);

  // 3) 无引号路径：UNC → 盘符 → POSIX
  out = out.replace(UNC_RE, PATH_PLACEHOLDER);
  out = out.replace(UNC_FWD_RE, PATH_PLACEHOLDER);
  out = out.replace(WIN_ABS_RE, PATH_PLACEHOLDER);
  // POSIX：以 /api/、/notify/ 开头的接口路径原样保留，其余绝对路径掩码
  out = out.replace(POSIX_ABS_RE, (m) => (KEEP_API_PATH_RE.test(m) ? m : PATH_PLACEHOLDER));

  // 4) 还原 URL
  out = out.replace(URL_TOKEN_RE, (_m, index: string) => urls[Number(index)] ?? _m);

  // 5) 截断（保留省略号，总长度不超过 maxLen）
  const limit = Number.isFinite(maxLen) && maxLen > 0 ? Math.floor(maxLen) : DEFAULT_MAX_LEN;
  if (out.length > limit) out = `${out.slice(0, Math.max(0, limit - 1))}${ELLIPSIS}`;

  return out;
}
