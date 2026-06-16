import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import "./index.css";

// Tauri 是桌面应用，不需要浏览器默认右键菜单，统一禁用避免露出 WebView 调试感。
document.addEventListener("contextmenu", (event) => {
  event.preventDefault();
});

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <App />
  </React.StrictMode>,
);
