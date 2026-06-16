// 应用列表持久化快照模块
// 首屏先读取上次扫描结果，后台再执行真实扫描刷新差异，减少冷启动等待。

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::models::InstalledApp;

const SNAPSHOT_VERSION: u32 = 1;
const ICON_SCHEME: &str = "viap-icon";

#[derive(Debug, Serialize, Deserialize)]
struct AppSnapshot {
    version: u32,
    apps: Vec<InstalledApp>,
}

fn snapshot_path() -> PathBuf {
    crate::storage::data_dir::get_data_dir()
        .join("cache")
        .join("app_snapshot.json")
}

fn encode_icon_path(icon_path: &str) -> String {
    // 自定义协议 URL 不能直接承载 Windows 路径中的空格、#、? 等字符，
    // 这里使用十六进制编码，避免引入新依赖并保持跨 WebView 行为稳定。
    let mut encoded = String::with_capacity(icon_path.len() * 2);
    for byte in icon_path.as_bytes() {
        encoded.push_str(&format!("{:02x}", byte));
    }
    encoded
}

pub fn decode_icon_path(encoded: &str) -> Option<String> {
    let clean = encoded.trim().trim_start_matches('/');
    if clean.len() % 2 != 0 || clean.is_empty() {
        return None;
    }

    let mut bytes = Vec::with_capacity(clean.len() / 2);
    for chunk_start in (0..clean.len()).step_by(2) {
        let chunk = &clean[chunk_start..chunk_start + 2];
        let byte = u8::from_str_radix(chunk, 16).ok()?;
        bytes.push(byte);
    }
    String::from_utf8(bytes).ok()
}

pub fn icon_url_for_path(icon_path: &str) -> String {
    if icon_path.trim().is_empty() {
        return String::new();
    }
    // Windows WebView2 对自定义协议使用 http://scheme.localhost/path 的来源形态，
    // 直接使用 viap-icon:// 会被当作未知 URL scheme，导致 img 加载失败。
    format!("http://{}.localhost/{}", ICON_SCHEME, encode_icon_path(icon_path))
}

pub fn attach_icon_urls(apps: &mut [InstalledApp]) {
    for app in apps {
        // icon_url 只描述“如何取图标”，真正提取交给 WebView 懒加载触发。
        if !app.display_icon.is_empty()
            && (app.icon_url.is_empty() || app.icon_url.starts_with("viap-icon://"))
        {
            app.icon_url = icon_url_for_path(&app.display_icon);
        }
    }
}

pub fn load_snapshot() -> Option<Vec<InstalledApp>> {
    let path = snapshot_path();
    let json = std::fs::read_to_string(path).ok()?;
    let snapshot = serde_json::from_str::<AppSnapshot>(&json).ok()?;
    if snapshot.version != SNAPSHOT_VERSION || snapshot.apps.is_empty() {
        return None;
    }
    let mut apps = snapshot.apps;
    attach_icon_urls(&mut apps);
    Some(apps)
}

pub fn save_snapshot(apps: &[InstalledApp]) {
    let path = snapshot_path();
    if let Some(parent) = path.parent() {
        if std::fs::create_dir_all(parent).is_err() {
            return;
        }
    }

    let mut apps_for_snapshot = apps.to_vec();
    attach_icon_urls(&mut apps_for_snapshot);
    // 快照不保存 Base64，避免 JSON 膨胀；图标由自定义协议按需读取磁盘缓存。
    for app in &mut apps_for_snapshot {
        app.icon_base64.clear();
    }

    let snapshot = AppSnapshot {
        version: SNAPSHOT_VERSION,
        apps: apps_for_snapshot,
    };
    let Ok(json) = serde_json::to_string(&snapshot) else {
        return;
    };

    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, json).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
        let _ = std::fs::remove_file(&tmp);
    }
}
