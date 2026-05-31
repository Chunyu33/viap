// 特殊目录检测与迁移模块
// 负责动态检测聊天类应用数据目录，并提供安全迁移入口

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sysinfo::System;

use crate::models::MigrationResult;
use crate::utils;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

#[cfg(windows)]
use winreg::enums::HKEY_CURRENT_USER;
#[cfg(windows)]
use winreg::RegKey;

/// 特殊文件夹状态
/// 前端用于展示检测结果和可迁移体积
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpecialFolder {
    pub name: String,
    pub current_path: String,
    pub is_detected: bool,
    pub size_mb: f64,
}

/// 动态检测聊天应用数据目录
///
/// 规则：
/// - 微信：注册表 FileSavePath（支持 MyDocument: 前缀）
/// - QQ/TIM：优先 Documents\Tencent Files，其次读取 QQ2012 安装路径推断
/// - 其他应用：使用 dirs crate 动态推断
pub fn detect_chat_app_data(app_name: &str) -> Option<PathBuf> {
    #[cfg(windows)]
    {
        let normalized = normalize_app_name(app_name);
        match normalized.as_str() {
            "wechat" => detect_wechat_path(),
            "qq" | "tim" => detect_qq_tim_path(),
            "wxwork" => detect_existing(default_special_path("wxwork")?),
            "dingtalk" => detect_existing(default_special_path("dingtalk")?),
            "feishu" | "lark" => detect_feishu_path(),
            _ => detect_existing(default_special_path(&normalized)?),
        }
    }

    #[cfg(not(windows))]
    {
        let _ = app_name;
        None
    }
}

/// 获取特殊目录状态列表（聊天应用 + 开发工具 + 浏览器缓存）
pub fn get_special_folders_status() -> Result<Vec<SpecialFolder>, String> {
    #[cfg(windows)]
    {
        let mut result = Vec::new();

        // 聊天应用
        for app_name in ["wechat", "qq", "tim", "wxwork", "dingtalk", "feishu"] {
            result.push(folder_status(app_name));
        }

        // 开发工具与浏览器缓存
        for app_name in ["chrome_cache", "edge_cache", "vscode_extensions", "npm_global"] {
            result.push(folder_status(app_name));
        }

        Ok(result)
    }

    #[cfg(not(windows))]
    {
        Ok(Vec::new())
    }
}

/// 获取单个特殊目录的状态（动态检测路径 + 大小）
#[cfg(windows)]
fn folder_status(app_name: &str) -> SpecialFolder {
    let detected = detect_chat_app_data(app_name);
    let fallback = default_special_path(app_name);

    let (current_path, is_detected, size_mb) = match detected {
        Some(path) => {
            let size_mb = calc_size_mb(&path);
            (path.to_string_lossy().to_string(), true, size_mb)
        }
        None => {
            let current_path = fallback
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_default();
            (current_path, false, 0.0)
        }
    };

    SpecialFolder {
        name: app_name.to_string(),
        current_path,
        is_detected,
        size_mb,
    }
}

/// 迁移特殊目录（安全工作流）
/// 1. 进程预检（目标应用必须已退出）
/// 2. 复用 migration::migrate_app 执行原子迁移与目录联接
pub fn migrate_special_folder(
    app_name: String,
    source_path: String,
    target_dir: String,
    cancel_flag: &Arc<AtomicBool>,
    app_handle: &tauri::AppHandle,
    force_overwrite: bool,
    user_confirmed_warning: bool,
) -> Result<MigrationResult, String> {
    #[cfg(windows)]
    {
        ensure_app_not_running(&app_name)?;
        crate::app_manager::migration::migrate_app(app_name, source_path, target_dir, cancel_flag, app_handle, crate::models::MigrationRecordType::LargeFolder, force_overwrite, user_confirmed_warning)
    }

    #[cfg(not(windows))]
    {
        let _ = (app_name, source_path, target_dir, cancel_flag, app_handle);
        Ok(MigrationResult {
            success: false,
            message: "此功能仅支持 Windows 系统".to_string(),
            new_path: None,
        })
    }
}

#[cfg(windows)]
fn normalize_app_name(app_name: &str) -> String {
    app_name.trim().to_lowercase()
}

#[cfg(windows)]
fn calc_size_mb(path: &Path) -> f64 {
    let bytes = utils::get_dir_size_safe(path);
    ((bytes as f64) / 1024.0 / 1024.0 * 100.0).round() / 100.0
}

#[cfg(windows)]
fn detect_existing(path: PathBuf) -> Option<PathBuf> {
    if path.exists() && path.is_dir() {
        Some(path)
    } else {
        None
    }
}

#[cfg(windows)]
fn detect_wechat_path() -> Option<PathBuf> {
    let reg_path = r"Software\Tencent\WeChat";
    let key = RegKey::predef(HKEY_CURRENT_USER).open_subkey(reg_path).ok()?;

    let raw_path: String = key.get_value("FileSavePath").ok()?;
    let resolved = resolve_wechat_save_path(&raw_path)?;
    detect_existing(resolved)
}

#[cfg(windows)]
fn resolve_wechat_save_path(raw: &str) -> Option<PathBuf> {
    let trimmed = raw.trim().trim_matches('"');
    if trimmed.is_empty() {
        return None;
    }

    if let Some(rest) = trimmed.strip_prefix("MyDocument:") {
        let doc = dirs::document_dir()?;
        let relative = rest.trim_start_matches(['\\', '/']);
        if relative.is_empty() {
            return Some(doc);
        }
        return Some(doc.join(relative));
    }

    Some(PathBuf::from(trimmed))
}

#[cfg(windows)]
fn detect_qq_tim_path() -> Option<PathBuf> {
    if let Some(documents) = dirs::document_dir() {
        let tencent_files = documents.join("Tencent Files");
        if tencent_files.exists() {
            return Some(tencent_files);
        }
    }

    let key = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(r"Software\Tencent\QQ2012")
        .ok()?;

    for value_name in ["InstallPath", "Install", "QQPath", "Executable"] {
        let install: String = key.get_value(value_name).unwrap_or_default();
        let install = install.trim().trim_matches('"');
        if install.is_empty() {
            continue;
        }

        let install_path = PathBuf::from(install);
        let guessed = install_path.join("Tencent Files");
        if guessed.exists() {
            return Some(guessed);
        }

        if install_path.exists() && install_path.is_dir() {
            return Some(install_path);
        }
    }

    None
}

#[cfg(windows)]
fn detect_feishu_path() -> Option<PathBuf> {
    // 候选路径按优先级排列：Roaming LarkShell → Local LarkShell → Roaming Feishu → Local Feishu
    let candidates: Vec<Option<PathBuf>> = vec![
        dirs::data_dir().map(|d| d.join("LarkShell")),
        dirs::data_local_dir().map(|d| d.join("LarkShell")),
        dirs::data_dir().map(|d| d.join("Feishu")),
        dirs::data_local_dir().map(|d| d.join("Feishu")),
        dirs::data_dir().map(|d| d.join("Lark")),
        dirs::data_local_dir().map(|d| d.join("Lark")),
    ];

    for candidate in candidates.into_iter().flatten() {
        if candidate.exists() && candidate.is_dir() {
            return Some(candidate);
        }
    }
    None
}

#[cfg(windows)]
fn default_special_path(app_name: &str) -> Option<PathBuf> {
    match app_name {
        "wechat" => dirs::document_dir().map(|d| d.join("WeChat Files")),
        "qq" | "tim" => dirs::document_dir().map(|d| d.join("Tencent Files")),
        "wxwork" => dirs::document_dir().map(|d| d.join("WXWork")),
        "dingtalk" => dirs::data_dir().map(|d| d.join("DingTalk")),
        "feishu" | "lark" => dirs::data_local_dir().map(|d| d.join("LarkShell")),
        "chrome_cache" => dirs::data_local_dir().map(|d| d.join(r"Google\Chrome\User Data\Default\Cache")),
        "edge_cache" => dirs::data_local_dir().map(|d| d.join(r"Microsoft\Edge\User Data\Default\Cache")),
        "vscode_extensions" => dirs::home_dir().map(|d| d.join(".vscode").join("extensions")),
        "npm_global" => {
            // 优先读取 npm config get prefix，支持用户自定义全局安装路径
            // 失败或返回空时回退到默认 %APPDATA%\npm\node_modules
            let from_npm = std::process::Command::new("npm")
                .args(["config", "get", "prefix"])
                .output()
                .ok()
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty() && s != "undefined")
                .map(|prefix| PathBuf::from(prefix).join("node_modules"));
            from_npm.or_else(|| dirs::data_dir().map(|d| d.join("npm").join("node_modules")))
        },
        _ => dirs::home_dir(),
    }
}

#[cfg(windows)]
fn expected_process_names(app_name: &str) -> &'static [&'static str] {
    match normalize_app_name(app_name).as_str() {
        "wechat" => &["wechat.exe"],
        "qq" => &["qq.exe"],
        "tim" => &["tim.exe"],
        "wxwork" => &["wxwork.exe"],
        "dingtalk" => &["dingtalk.exe"],
        "feishu" | "lark" => &["feishu.exe", "lark.exe"],
        "chrome_cache" => &["chrome.exe"],
        "edge_cache" => &["msedge.exe"],
        "vscode_extensions" => &["code.exe"],
        _ => &[],
    }
}

#[cfg(windows)]
fn ensure_app_not_running(app_name: &str) -> Result<(), String> {
    let expected = expected_process_names(app_name);
    if expected.is_empty() {
        return Ok(());
    }

    let mut system = System::new_all();
    system.refresh_all();

    let mut running: Vec<String> = Vec::new();
    for process in system.processes().values() {
        let name = process.name().to_string_lossy().to_lowercase();
        if expected.iter().any(|candidate| name == *candidate) {
            running.push(name);
        }
    }

    running.sort();
    running.dedup();

    if running.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "检测到应用仍在运行，请先关闭后再迁移: {}",
            running.join(", ")
        ))
    }
}
