import { BrowserManager } from "../src/browser/manager.js";
import { HttpBridge } from "../src/bridge/http-server.js";
import { loadConfig, getConfig } from "../src/config/index.js";
import { registerLocale, getPanelStrings } from "../src/i18n/index.js";
import en from "../src/i18n/en.js";
import zh from "../src/i18n/zh.js";
import { readFileSync } from "fs";

async function main() {
  registerLocale("en", en);
  registerLocale("zh", zh);
  loadConfig();

  // 动态端口
  const bridge = new HttpBridge(0);
  const port = await bridge.start();

  const manager = new BrowserManager("");

  // 注册截图回调
  bridge.onScreenshot(async (rect) => {
    const page = await manager.getActivePage();
    const clip = rect ? { x: rect.x, y: rect.y, width: rect.w, height: rect.h } : undefined;
    const buffer = await page.screenshot({ fullPage: false, clip, type: "png" });
    return buffer.toString("base64");
  });

  const config = getConfig();
  const panelJs = readFileSync("dist/panel/panel.js", "utf-8");
  const panelConfig = JSON.stringify({ port, strings: getPanelStrings(config.panelLang) });
  manager.setPanelScript(`window.__PW_CONFIG__ = ${panelConfig};\n${panelJs}`);

  console.log(`Bridge on port ${port}`);
  console.log("Browser opening...");
  console.log("Magic Panel button: ✦ at bottom-right corner");
  console.log("Press Ctrl+C to exit\n");

  await manager.launch(false);
  await manager.navigate("https://example.com");

  await new Promise(() => {});
}

main().catch(console.error);
