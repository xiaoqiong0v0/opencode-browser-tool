import { BRIDGE, SESSION_ID } from "./config";

/** POST 请求到 Bridge（自动附加 sessionId） */
export async function post(url: string, body?: any): Promise<any> {
  const resp = await fetch(BRIDGE + url, {
    method: "POST",
    headers: { "Content-Type": "application/json" },
    body: JSON.stringify({ ...body, sessionId: SESSION_ID }),
  });
  return resp.json();
}

/** HTML 转义 */
export function escHtml(str: string): string {
  return str.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
}
