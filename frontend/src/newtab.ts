// 新标签页入口:应用主题(跟随系统/手动),背景随主题变化
import { createIcons, Globe } from "lucide";
import { initTheme } from "./theme";

void initTheme();
// lucide 图标替换(新标签页 globe)
createIcons({ icons: { Globe } });
