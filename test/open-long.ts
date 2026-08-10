import { BrowserManager } from "../src/browser/manager.js";
import { HttpBridge } from "../src/bridge/http-server.js";
import { loadConfig, getConfig } from "../src/config/index.js";
import { registerLocale, getPanelStrings } from "../src/i18n/index.js";
import en from "../src/i18n/en.js";
import zh from "../src/i18n/zh.js";
import { readFileSync } from "fs";
import { resolve } from "path";

async function main() {
  registerLocale("en", en);
  registerLocale("zh", zh);
  loadConfig();

  const bridge = new HttpBridge(0);
  const port = await bridge.start();

  const manager = new BrowserManager("");

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

  // 打开长页面
  const longPage = "file://" + resolve("test/long-page.html");
  console.log(`Bridge: ${port}`);
  console.log(`Opening: ${longPage}`);
  console.log("Click ✦ → 📷 to test full-page screenshot and annotations\n");

  await manager.launch(false);
  await manager.navigate(longPage);

  await new Promise(() => {});
}

main().catch(console.error);
