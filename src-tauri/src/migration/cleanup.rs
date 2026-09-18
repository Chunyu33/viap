// 目录清理子模块
// 强制删除目录（处理只读属性与部分文件被占用的情况）

use std::fs;
use std::path::{Path, PathBuf};

use walkdir::WalkDir;

pub(crate) fn remove_directory_robust(path: &Path) -> Result<(), std::io::Error> {
    match fs::remove_dir_all(path) {
        Ok(_) => return Ok(()),
        Err(e) if e.kind() != std::io::ErrorKind::PermissionDenied => return Err(e),
        Err(_) => {}
    }

    // 因只读文件导致失败：遍历清除只读属性后逐个删除。
    // 尽量删除所有可删文件：单个文件被进程占用（如资源管理器加载的
    // shell extension DLL）不应阻止其余文件清理，残留目录只保留真正
    // 被占用的文件，便于用户稍后手动处理。
    let (first_file_error, first_dir_error) = remove_directory_contents(path);

    // 最终删除根目录；优先用文件失败信息（更具体，能指出被占用项）
    fs::remove_dir(path).map_err(|e| {
        if let Some((failed_file, file_err)) = first_file_error {
            std::io::Error::new(
                file_err.kind(),
                format!(
                    "文件删除失败（被占用：{}，原因：{}）；根目录删除失败：{}",
                    failed_file.display(), file_err, e
                ),
            )
        } else if let Some((failed_dir, dir_err)) = first_dir_error {
            std::io::Error::new(
                dir_err.kind(),
                format!(
                    "子目录删除失败（目录：{}，原因：{}）；根目录删除失败：{}",
                    failed_dir.display(), dir_err, e
                ),
            )
        } else {
            e
        }
    })
}

/// 逐个删除目录内容（不删根目录），返回（首个文件失败, 首个目录失败）
///
/// 单独抽出便于单测覆盖：目录联接既不是 is_file() 也不是 is_dir()，
/// 必须按重解析点处理，否则备份目录会因为残留联接而删不掉。
pub(crate) fn remove_directory_contents(
    path: &Path,
) -> (Option<(PathBuf, std::io::Error)>, Option<(PathBuf, std::io::Error)>) {
    let mut first_file_error: Option<(PathBuf, std::io::Error)> = None;
    let mut first_dir_error: Option<(PathBuf, std::io::Error)> = None;

    for entry in WalkDir::new(path).contents_first(true).into_iter().filter_map(|e| e.ok()) {
        let entry_path = entry.path();
        // 清除只读属性，否则 remove_file / remove_dir 会失败
        if let Ok(mut perms) = fs::metadata(entry_path).map(|m| m.permissions()) {
            perms.set_readonly(false);
            let _ = fs::set_permissions(entry_path, perms);
        }
        if crate::utils::is_junction(entry_path) {
            // 目录联接：remove_dir 只删链接本身，不会递归删除链接目标
            if let Err(e) = fs::remove_dir(entry_path) {
                if first_dir_error.is_none() {
                    first_dir_error = Some((entry_path.to_path_buf(), e));
                }
            }
        } else if entry_path.is_file() {
            // 文件被进程持有时删除失败：记录第一个失败，继续清理其余文件
            if let Err(e) = fs::remove_file(entry_path) {
                if first_file_error.is_none() {
                    first_file_error = Some((entry_path.to_path_buf(), e));
                }
            }
        } else if entry_path.is_dir() && entry_path != path {
            if let Err(e) = fs::remove_dir(entry_path) {
                // 记录第一个失败的子目录，用于最终错误消息诊断
                if first_dir_error.is_none() {
                    first_dir_error = Some((entry_path.to_path_buf(), e));
                }
            }
        }
    }

    (first_file_error, first_dir_error)
}

/// 将目录标记为重启时自动删除（MoveFileEx + MOVEFILE_DELAY_UNTIL_REBOOT）
///
/// 用于备份目录清理失败（文件被 explorer 等服务占用）的场景：
/// 占用句柄会在重启后释放，系统启动阶段自动删除整个目录树。
/// 需要管理员权限（写入 Session Manager 的 PendingFileRenameOperations），
/// 失败返回 false，调用方降级为用户手动删除提示。
#[cfg(windows)]
pub(crate) fn schedule_remove_on_reboot(path: &Path) -> bool {
    // 与复制引擎一致：原生 API 需要扩展长度路径，否则深层备份目录超过 260 字符时静默失败
    let path_wide = crate::utils::to_extended_length_wide(path);
    unsafe {
        // 目标为 NULL + MOVEFILE_DELAY_UNTIL_REBOOT：重启后删除整个目录树
        windows::Win32::Storage::FileSystem::MoveFileExW(
            windows::core::PCWSTR(path_wide.as_ptr()),
            windows::core::PCWSTR(std::ptr::null()),
            windows::Win32::Storage::FileSystem::MOVEFILE_DELAY_UNTIL_REBOOT,
        )
        .is_ok()
    }
}

/// 非 Windows 平台回退：无重启删除机制，返回 false
#[cfg(not(windows))]
pub(crate) fn schedule_remove_on_reboot(_path: &Path) -> bool {
    false
}

/// 清理源父目录下遗留的 Viap 迁移备份目录（.viap_migration_backup_*）
///
/// 迁移后备份清理失败（文件被占用）会遗留多余副本；还原成功后源目录已恢复，
/// 此时残留备份是多余数据，一并尝试清理。仍被占用的文件标记为重启后删除。
///
/// 安全保护：跳过当前进程正在进行的迁移备份（备份名含创建进程 pid），
/// 避免与并发迁移任务互相干扰。
pub(crate) fn cleanup_leftover_backups(parent: &Path) {
    let current_pid = std::process::id();
    let Ok(entries) = fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !name.starts_with(".viap_migration_backup_") {
            continue;
        }
        // 解析备份名中的创建进程 pid，跳过本进程正在进行的迁移备份
        let pid_part = name
            .strip_prefix(".viap_migration_backup_")
            .and_then(|rest| rest.split('_').next())
            .and_then(|p| p.parse::<u32>().ok());
        if pid_part == Some(current_pid) {
            continue;
        }
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        match remove_directory_robust(&path) {
            Ok(_) => {
                log_warn!("migration", "已清理历史残留备份: {}", path.display());
            }
            Err(e) => {
                log_warn!("migration", "清理残留备份失败 {}: {}", path.display(), e);
                // 被占用文件重启后自动删除，避免残留目录长期占用空间
                if schedule_remove_on_reboot(&path) {
                    log_warn!("migration", "已安排残留备份重启后删除: {}", path.display());
                }
            }
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    /// 备份目录里含目录联接时，降级删除路径也必须能清干净
    /// （联接既不是 is_file() 也不是 is_dir()，早前会被整个跳过）
    #[test]
    fn contents_removal_handles_directory_links() {
        let suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("系统时间应有效")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("viap-cleanup-{suffix}"));
        let backup = root.join("backup");
        let external = root.join("external");
        std::fs::create_dir_all(backup.join("nested")).expect("创建备份目录失败");
        std::fs::create_dir_all(&external).expect("创建外部目录失败");
        std::fs::write(backup.join("nested").join("a.bin"), b"a").expect("写入文件失败");
        std::fs::write(external.join("keep.bin"), b"keep").expect("写入外部文件失败");
        junction::create(&external, &backup.join("inner-link")).expect("创建目录联接失败");

        let (file_error, dir_error) = remove_directory_contents(&backup);
        assert!(dir_error.is_none(), "目录联接必须能被删除: {:?}", dir_error.map(|(p, e)| (p, e.to_string())));
        assert!(file_error.is_none());

        remove_directory_robust(&backup).expect("删除备份目录失败");
        assert!(!crate::utils::is_junction(&backup.join("inner-link")));
        // 联接指向的外部数据不能被误删
        assert!(external.join("keep.bin").is_file());

        std::fs::remove_dir_all(&root).ok();
    }
}