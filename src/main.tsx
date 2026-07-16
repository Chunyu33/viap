import React from "react";
import ReactDOM from "react-dom/client";
import { invoke } from "@tauri-apps/api/core";
import App from "./App";
import "./index.css";
import { applyFontSize } from "./utils/fontSize";
import { bootstrapUserSettings } from "./utils/userSettings";

// Tauri 是桌面应用，不需要浏览器默认右键菜单，统一禁用避免露出 WebView 调试感。
document.addEventListener("contextmenu", (event) => {
  event.preventDefault();
});

async function bootstrapApplication() {
  // 先初始化便携版数据目录和旧安装版导入，再挂载页面避免读取到错误配置。
  const storage = await invoke<{ imported_legacy_data: boolean }>('initialize_storage')
    .catch(() => null);
  if (storage?.imported_legacy_data) {
    try {
      // 设置页可能稍后才挂载，用 sessionStorage 把一次性导入结果交给它展示。
      sessionStorage.setItem('viap_storage_imported', '1');
    } catch {
      // WebView 会话存储不可用时不影响数据导入本身。
    }
  }
  const settings = await bootstrapUserSettings();
  applyFontSize(settings.fontSizePx);
  ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
    <React.StrictMode>
      <App />
    </React.StrictMode>,
  );
}

void bootstrapApplication();
