// 迁移应用元数据持久化模块（兜底机制）
//
// 问题：绿色软件（无注册表条目）迁移后安装目录变为联接，
// Tier3 扫描器可能因各种原因无法发现它们，导致应用从列表消失。
//
// 兜底：迁移成功时将应用元数据写入独立 JSON 文件，
// 应用列表加载时读取并补全扫描器遗漏的已迁移应用。

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::models::InstalledApp;
use super::data_dir::ensure_data_dir;

/// 迁移应用元数据条目
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigratedAppEntry {
    pub app_name: String,
    pub original_path: String,
    pub target_path: String,
}

#[derive(Debug, Serialize, Deserialize)]
struct MigratedAppStorage {
    apps: Vec<MigratedAppEntry>,
}

fn metadata_file_path() -> PathBuf {
    ensure_data_dir().join("migrated_apps.json")
}

/// 加载全部迁移应用元数据
fn load_all() -> Vec<MigratedAppEntry> {
    let path = metadata_file_path();
    if !path.exists() {
        return Vec::new();
    }
    fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str::<MigratedAppStorage>(&s).ok())
        .map(|s| s.apps)
        .unwrap_or_default()
}

/// 原子保存元数据列表
fn save_all(apps: &[MigratedAppEntry]) -> Result<(), String> {
    let path = metadata_file_path();
    let storage = MigratedAppStorage { apps: apps.to_vec() };
    let json = serde_json::to_string_pretty(&storage)
        .map_err(|e| format!("序列化元数据失败: {}", e))?;
    fs::write(&path, &json)
        .map_err(|e| format!("写入元数据文件失败: {}", e))?;
    Ok(())
}

/// 迁移成功后新增元数据条目（去重写入）
pub fn add_migrated_app(app_name: &str, original_path: &str, target_path: &str) {
    let mut apps = load_all();
    apps.retain(|a| a.original_path != original_path);
    apps.push(MigratedAppEntry {
        app_name: app_name.to_string(),
        original_path: original_path.to_string(),
        target_path: target_path.to_string(),
    });
    if let Err(e) = save_all(&apps) {
        eprintln!("[viap/metadata] 保存迁移应用元数据失败: {}", e);
    }
}

/// 恢复/清理后移除元数据条目
pub fn remove_migrated_app(original_path: &str) {
    let mut apps = load_all();
    let before = apps.len();
    apps.retain(|a| a.original_path != original_path);
    if apps.len() != before {
        let _ = save_all(&apps);
    }
}

/// 在目录中查找主 exe（用于图标提取）
/// 跳过安装器/卸载器命名，优先选择与目录同名的 exe
fn find_main_exe(dir: &Path) -> Option<PathBuf> {
    let dir_name = dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("")
        .to_lowercase();
    let mut candidates: Vec<PathBuf> = Vec::new();

    for entry in fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        if path.extension().and_then(|e| e.to_str())?.to_lowercase() != "exe" {
            continue;
        }
        let file_name = path
            .file_stem()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_lowercase();
        // 跳过明显是安装器/卸载器的 exe
        if is_installer_like_name(&file_name) {
            continue;
        }
        // 与目录同名的 exe 优先返回
        if file_name == dir_name {
            return Some(path);
        }
        candidates.push(path);
    }
    candidates.into_iter().next()
}

/// 判断文件名是否像安装器/卸载器
fn is_installer_like_name(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.contains("setup")
        || lower.contains("install")
        || lower.contains("update")
        || lower.contains("uninst")
        || lower.contains("unins0")
        || lower.contains("unins1")
}

/// 生成兜底应用列表
///
/// 从元数据构造 InstalledApp，仅保留：
/// 1. 原路径仍为目录联接（迁移有效）
/// 2. 扫描结果中不存在（避免重复）
/// 同时生成图标 URL，确保前端能按需懒加载真实图标
pub fn generate_failsafe_apps(existing: &HashSet<String>) -> Vec<InstalledApp> {
    let mut result = Vec::new();
    for entry in load_all() {
        let original = Path::new(&entry.original_path);
        // 联接已失效（被手动删除或恢复）→ 跳过
        let is_symlink = original.symlink_metadata()
            .map(|m| m.file_type().is_symlink())
            .unwrap_or(false);
        if !is_symlink {
            continue;
        }
        let loc_key = entry.original_path.to_lowercase();
        if existing.contains(&loc_key) {
            continue;
        }
        // 通过联接查找主 exe 以提取图标
        let display_icon = find_main_exe(original)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        let icon_url = crate::app_manager::snapshot::icon_url_for_path(&display_icon);
        result.push(InstalledApp {
            display_name: entry.app_name,
            install_location: entry.original_path,
            display_icon,
            estimated_size: 0,
            icon_base64: String::new(),
            icon_url,
            registry_path: String::new(),
            publisher: String::new(),
        });
    }
    result
}
