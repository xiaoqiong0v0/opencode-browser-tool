// 本地 HTTP 服务 API 客户端(面板 / 工具栏 Webview 共用):POST + 实例鉴权头 `x-bt-token`
// 端口与令牌在 shell 实例创建期确定,首次调用时通过 `invoke` 读取并缓存(同一 Webview 内共享)
import { invoke } from "@tauri-apps/api/core";

/** 服务端口缓存(0 = 尚未读取) */
let servicePort = 0;
/** API 令牌缓存(null = 尚未读到) */
let apiToken: string | null = null;
/** 令牌读取中的在途 Promise:并发请求共享同一次 invoke */
let apiTokenPromise: Promise<string | null> | null = null;

/**
 * 获取并缓存 API 令牌(fetch `/api/*` 时作为 `x-bt-token` 头)
 *
 * 失败不阻断调用方:控制台打出明确错误并返回 null(后续请求会 401),下次调用重试。
 *
 * @param tag 日志前缀(区分调用方,如 `panel` / `toolbar`)
 * @returns 令牌;读取失败为 null
 */
export async function getApiToken(tag = "api"): Promise<string | null> {
  if (apiToken !== null) return apiToken;
  if (!apiTokenPromise) {
    apiTokenPromise = invoke<string>("api_token")
      .then((token) => {
        apiToken = token;
        return token as string | null;
      })
      .catch((e) => {
        console.error(`[${tag}] 获取 API 令牌失败(api_token invoke),/api/* 将返回 401:`, e);
        return null;
      })
      .finally(() => {
        // 失败时清空在途 Promise,允许下次调用重试;成功则保留缓存值
        if (apiToken === null) apiTokenPromise = null;
      });
  }
  return apiTokenPromise;
}

/**
 * 调用本地 HTTP 服务 API(POST JSON,带实例鉴权头)
 *
 * @param path 接口路径(如 `/api/downloads`)
 * @param body 请求体(默认 `{}`)
 * @param tag 日志前缀(区分调用方,如 `panel` / `toolbar`)
 * @returns 响应 JSON(非 2xx 只记日志,由调用方按 `success` 字段判断)
 */
export async function postApi(path: string, body?: unknown, tag = "api"): Promise<any> {
  const port = servicePort || (await invoke<number>("panel_service_port"));
  servicePort = port;
  const token = await getApiToken(tag);
  const headers: Record<string, string> = { "Content-Type": "application/json" };
  if (token) headers["x-bt-token"] = token;
  const res = await fetch(`http://127.0.0.1:${port}${path}`, {
    method: "POST",
    headers,
    body: JSON.stringify(body || {}),
  });
  if (!res.ok) {
    console.error(`[${tag}] api ${path} failed: HTTP ${res.status}${token ? "" : "(no api token)"}`);
  }
  return res.json();
}
