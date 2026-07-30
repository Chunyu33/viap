// Viap 工具函数模块
// 提供跨模块共享的文件系统操作辅助函数

use std::path::{Path, PathBuf};
#[cfg(windows)]
use std::fs;
#[cfg(windows)]
use sysinfo::Disks;
use walkdir::WalkDir;
use std::sync::atomic::{AtomicBool, Ordering};

/// 全局恢复锁：确保同一时刻只有一个恢复任务在运行
/// 前端 restoringId 仅阻止 UI 重复点击，无法阻止快速双击或来自不同入口的并发 invoke
pub static RESTORE_LOCK: AtomicBool = AtomicBool::new(false);

/// RAII 锁守卫：在函数任意返回路径（包括 ? 提前返回）自动释放全局恢复锁
pub struct RestoreLockGuard;
impl Drop for RestoreLockGuard {
    fn drop(&mut self) {
        RESTORE_LOCK.store(false, Ordering::SeqCst);
    }
}

/// 尝试获取恢复锁，返回 RAII 守卫；若已被占用则返回错误信息
pub fn try_acquire_restore_lock() -> Result<RestoreLockGuard, String> {
    if RESTORE_LOCK.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_err() {
        return Err("另一个恢复任务正在进行中，请等待完成后再试".to_string());
    }
    Ok(RestoreLockGuard)
}

/// 检测路径是否为 Junction（目录联接）
///
/// # 技术说明
/// Windows Junction 是一种重解析点（Reparse Point），
/// 通过检查 FILE_ATTRIBUTE_REPARSE_POINT 标志来判断
#[cfg(windows)]
pub fn is_junction(path: &Path) -> bool {
    use std::os::windows::fs::MetadataExt;
    if let Ok(metadata) = fs::symlink_metadata(path) {
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        return (metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT) != 0;
    }
    false
}

#[cfg(not(windows))]
pub fn is_junction(_path: &Path) -> bool { false }

/// 获取 Junction 的目标路径
///
/// 使用 fs::read_link 读取符号链接/Junction 的目标，
/// 并去除 Windows 路径可能带有的 `\\?\` 前缀
#[cfg(windows)]
pub fn get_junction_target(path: &Path) -> Option<String> {
    if is_junction(path) {
        if let Ok(target) = fs::read_link(path) {
            let target_str = target.to_string_lossy().to_string();
            return Some(target_str.trim_start_matches("\\\\?\\").to_string());
        }
    }
    None
}

#[cfg(not(windows))]
pub fn get_junction_target(_path: &Path) -> Option<String> { None }

/// 权限无关的目录大小计算
///
/// 使用 WalkDir 遍历，跳过无权限访问的文件/目录，只统计可读文件。
/// 替代 fs_extra::get_size —— 后者在单个不可读条目上直接失败。
pub fn get_dir_size_safe(path: &Path) -> u64 {
    WalkDir::new(path)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter_map(|e| e.metadata().ok())
        .filter(|m| m.is_file())
        .map(|m| m.len())
        .sum()
}

/// 展开路径中的环境变量（如 %APPDATA%/subdir → C:/Users/.../AppData/Roaming/subdir）
pub fn expand_env_vars(path_str: &str) -> String {
    let mut result = String::with_capacity(path_str.len());
    let mut remaining = path_str;
    while let Some(start) = remaining.find('%') {
        result.push_str(&remaining[..start]);
        remaining = &remaining[start + 1..];
        if let Some(end) = remaining.find('%') {
            let var_name = &remaining[..end];
            let expanded = std::env::var(var_name)
                .unwrap_or_else(|_| format!("%{}%", var_name));
            result.push_str(&expanded);
            remaining = &remaining[end + 1..];
        } else {
            // 孤立的 %，原样保留
            result.push('%');
            result.push_str(remaining);
            remaining = "";
            break;
        }
    }
    result.push_str(remaining);
    result
}

/// 检查目标盘是否有足够空间容纳还原文件
/// 要求可用空间 >= 文件大小 × 1.1（10% 缓冲）
/// 返回 (可用空间, 所需空间) 或错误
pub fn check_disk_space_for_restore(target_dir: &Path, required_bytes: u64) -> Result<(u64, u64), String> {
    let required_with_buffer = (required_bytes as f64 * 1.1) as u64;

    let target_str = target_dir.to_string_lossy();
    let drive_prefix = if target_str.len() >= 2 && target_str.as_bytes()[1] == b':' {
        format!("{}\\", &target_str[..2])
    } else {
        return Err("无法确定目标盘符".to_string());
    };

    let disks = Disks::new_with_refreshed_list();
    for disk in disks.list() {
        let mount = disk.mount_point().to_string_lossy().to_string();
        if mount.starts_with(&drive_prefix[..1]) || mount.eq_ignore_ascii_case(&drive_prefix) {
            let available = disk.available_space();
            if available < required_with_buffer {
                return Err(format!(
                    "目标磁盘空间不足：需要 {} 字节（含 10% 缓冲），可用 {} 字节",
                    required_with_buffer, available
                ));
            }
            return Ok((available, required_with_buffer));
        }
    }

    Err(format!("未找到目标磁盘: {}", drive_prefix))
}

/// 获取实际的 app_data_templates.json 路径
pub fn app_data_templates_path(data_dir: &Path) -> PathBuf {
    data_dir.join("app_data_templates.json")
}

/// 获取 custom_folders.json 路径
pub fn custom_folders_path(data_dir: &Path) -> PathBuf {
    data_dir.join("custom_folders.json")
}

/// 获取 migration_history.json 路径
pub fn history_file_path(data_dir: &Path) -> PathBuf {
    data_dir.join("migration_history.json")
}
