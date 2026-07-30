//! 大文件夹大小扫描。
//!
//! 扫描是纯阻塞文件系统 IO，必须与 Tauri 命令和列表构造解耦。
//! 本模块只负责接收快照、后台统计并逐项发送事件，避免页面等待整批结果。

use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use tauri::{AppHandle, Emitter};
use walkdir::WalkDir;

use crate::models::{LargeFolder, LargeFolderSizeEvent, LargeFolderType};

/// 启动系统/自定义目录的异步大小扫描。
pub fn start_folder_size_scan(
    folders: Vec<LargeFolder>,
    app_handle: AppHandle,
    scan_id: Option<String>,
) -> Result<(), String> {
    let candidates = folders
        .into_iter()
        .filter(|folder| folder.folder_type != LargeFolderType::AppData)
        .collect();
    spawn_scan(candidates, app_handle, scan_id)
}

/// 启动用户主动触发的应用数据大小扫描。
pub fn start_app_data_size_scan(
    folders: Vec<LargeFolder>,
    app_handle: AppHandle,
    scan_id: Option<String>,
) -> Result<(), String> {
    let candidates = folders
        .into_iter()
        .filter(|folder| folder.folder_type == LargeFolderType::AppData)
        .collect();
    spawn_scan(candidates, app_handle, scan_id)
}

fn spawn_scan(
    folders: Vec<LargeFolder>,
    app_handle: AppHandle,
    scan_id: Option<String>,
) -> Result<(), String> {
    thread::Builder::new()
        .name("large-folder-size-scan".to_string())
        .spawn(move || scan_folders(folders, app_handle, scan_id))
        .map(|_| ())
        .map_err(|error| format!("启动目录大小扫描失败: {}", error))
}

fn scan_folders(folders: Vec<LargeFolder>, app_handle: AppHandle, scan_id: Option<String>) {
    for folder in folders {
        let size = folder_size(&folder);
        let _ = app_handle.emit("large-folder-size", LargeFolderSizeEvent {
            folder_id: folder.id,
            size,
            scan_id: scan_id.clone(),
        });
    }
}

fn folder_size(folder: &LargeFolder) -> u64 {
    let Some(path) = scan_path(folder) else { return 0; };
    if !path.is_dir() { return 0; }
    calculate_directory_size(&path)
}

fn scan_path(folder: &LargeFolder) -> Option<PathBuf> {
    if !folder.exists {
        return None;
    }
    if folder.is_junction {
        return folder.junction_target.as_ref().map(PathBuf::from);
    }
    Some(PathBuf::from(&folder.path))
}

/// 使用单次 metadata 读取并明确不跟随链接，避免重复遍历和递归到外部目录。
fn calculate_directory_size(path: &Path) -> u64 {
    let mut total = 0u64;
    let mut entries = 0u32;
    for entry in WalkDir::new(path).follow_links(false).into_iter().filter_map(Result::ok) {
        if entry.file_type().is_file() {
            if let Ok(metadata) = entry.metadata() {
                total = total.saturating_add(metadata.len());
            }
        }
        entries += 1;
        // 大目录扫描主动让出调度，降低 HDD/CPU 持续满载时对 WebView 响应的影响。
        if entries % 2048 == 0 {
            thread::yield_now();
            thread::sleep(Duration::from_millis(1));
        }
    }
    total
}
