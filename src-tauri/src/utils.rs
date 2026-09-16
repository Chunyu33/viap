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
    let available = available_space_for_path(target_dir)
        .ok_or_else(|| format!("未找到目标磁盘: {}", target_dir.display()))?;

    if available < required_with_buffer {
        return Err(format!(
            "目标磁盘空间不足：需要 {} 字节（含 10% 缓冲），可用 {} 字节",
            required_with_buffer, available
        ));
    }
    Ok((available, required_with_buffer))
}

/// 查询路径所在卷的可用空间
///
/// 挂载点必须按"完整路径分隔边界"匹配，否则以盘符首字母比较会把 `C:\` 误判成
/// 任何以 C 开头的挂载点（如 CD-ROM 或自定义挂载目录），从而读到错误的可用空间。
pub fn available_space_for_path(path: &Path) -> Option<u64> {
    let path_upper = path.to_string_lossy().to_uppercase();
    Disks::new_with_refreshed_list()
        .list()
        .iter()
        .filter_map(|disk| {
            let mount = disk.mount_point().to_string_lossy().to_uppercase();
            let mount_clean = mount.trim_end_matches('\\');
            if mount_clean.is_empty() {
                return None;
            }
            // 匹配完整路径分隔边界，避免 C: 误匹配 CD: 或 C:\Mount\Disk2
            let is_match = path_upper == mount_clean
                || (path_upper.starts_with(mount_clean)
                    && path_upper.as_bytes().get(mount_clean.len()) == Some(&b'\\'));
            if is_match {
                Some((mount_clean.len(), disk.available_space()))
            } else {
                None
            }
        })
        // 多个挂载点匹配时选最长（最具体）的那个
        .max_by_key(|(length, _)| *length)
        .map(|(_, space)| space)
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

// ============================================================================
// 长路径支持
// ============================================================================

/// 把路径转换为 Win32 扩展长度（`\\?\`）宽字符形式，供原生 API 调用使用
///
/// # 为什么需要
///
/// Viap 的可执行文件没有声明 `longPathAware`，而注册表的长路径策略只对声明过的
/// 进程生效，因此 `CopyFileExW` / `MoveFileExW` 这类 Win32 API 在路径超过 260 字符时
/// 会返回 `ERROR_PATH_NOT_FOUND(3)`。Rust 标准库内部会自动加前缀，原生调用不会——
/// 这就是 Yarn / npm 缓存（目录名很长）迁移报「错误码 3」的根因。
///
/// # 边界
///
/// 扩展长度形式不做路径归一化：`.` / `..` 不会被解析，相对路径也无法使用，
/// 因此只对「绝对路径且不含这两个分量」的路径加前缀，其余情况退回普通形式，
/// 与修复前的行为保持一致。
pub fn to_extended_length_wide(path: &Path) -> Vec<u16> {
    let text = path.to_string_lossy().replace('/', "\\");
    extend_win32_path(&text)
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect()
}

/// 为绝对路径加 `\\?\` 前缀；UNC 路径需要 `\\?\UNC\` 形式
fn extend_win32_path(text: &str) -> String {
    if !is_extendable_absolute_path(text) {
        return text.to_string();
    }
    if text.starts_with(r"\\?\") {
        return text.to_string();
    }
    if let Some(unc_rest) = text.strip_prefix(r"\\") {
        return format!(r"\\?\UNC\{}", unc_rest);
    }
    format!(r"\\?\{}", text)
}

/// 判断路径是否可以安全地套用扩展长度前缀
fn is_extendable_absolute_path(text: &str) -> bool {
    let bytes = text.as_bytes();
    let has_drive_prefix = bytes.len() >= 3 && bytes[1] == b':' && bytes[2] == b'\\';
    let is_unc_prefix = text.starts_with(r"\\");
    if !has_drive_prefix && !is_unc_prefix {
        return false;
    }
    !text.split('\\').any(|part| part == "." || part == "..")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extended_length_path_covers_drive_unc_and_fallbacks() {
        let to_text = |path: &str| {
            let wide = to_extended_length_wide(Path::new(path));
            String::from_utf16_lossy(&wide[..wide.len() - 1])
        };

        // 盘符绝对路径：统一加 \\?\ 前缀，并归一化正斜杠
        assert_eq!(to_text(r"C:\a\b.bin"), r"\\?\C:\a\b.bin");
        assert_eq!(to_text("C:/a/b.bin"), r"\\?\C:\a\b.bin");
        // 已有前缀不重复添加；UNC 路径使用 \\?\UNC\ 形式
        assert_eq!(to_text(r"\\?\C:\a"), r"\\?\C:\a");
        assert_eq!(to_text(r"\\server\share\a"), r"\\?\UNC\server\share\a");
        // 相对路径和含 . / .. 的路径必须保持原样，否则会变成非法路径
        assert_eq!(to_text(r"a\b"), r"a\b");
        assert_eq!(to_text(r"C:\a\..\b"), r"C:\a\..\b");
    }
    #[test]
    fn available_space_lookup_matches_volume_boundaries() {
        // 临时目录所在卷必须能查到可用空间
        let available = available_space_for_path(&std::env::temp_dir());
        assert!(available.map(|value| value > 0).unwrap_or(false));

        // 不存在的盘符不能匹配到别的卷（旧实现按首字母匹配会误判）
        assert!(available_space_for_path(Path::new(r"Z:\not-exist")).is_none());
    }
}