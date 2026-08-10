// 读取插件注入的配置
interface PWConfig {
  port: number;
  sessionId?: string;
  strings: Record<string, string>;
}
export const cfg: PWConfig = (window as any).__PW_CONFIG__ || { port: 3456, sessionId: "", strings: {} };
export const BRIDGE = `http://localhost:${cfg.port}`;
export const SESSION_ID = cfg.sessionId || "";
export const s = (k: string) => cfg.strings[k] || k;
