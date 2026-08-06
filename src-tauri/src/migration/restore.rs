// 目录恢复子模块
// 将已迁移目录从目标位置恢复到原路径（还原流程）

use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use crate::migration::cleanup::{cleanup_leftover_backups, remove_directory_robust};
use crate::migration::copy_engine::{build_copy_plan_with_progress, copy_dir_with_progress};
use crate::migration::links::rollback_restore_link;
use crate::migration::{emit_progress, format_bytes};

pub(crate) struct RestoreDirectoryResult {
    /// 已恢复到原路径的字节数，用于成功提示或后续日志。
    pub(crate) restored_size: u64,
    /// 目标副本清理失败时不影响数据完整性，但需要提示调用方。
    pub(crate) cleanup_warning: Option<String>,
}

pub(crate) fn restore_directory_with_progress(
    original_path: &Path,
    target_path: &Path,
    task_id: &str,
    app_handle: &tauri::AppHandle,
) -> Result<RestoreDirectoryResult, String> {
    let cancel_flag = Arc::new(AtomicBool::new(false));
    let restore_plan = build_copy_plan_with_progress(
        target_path,
        original_path,
        task_id,
        &cancel_flag,
        app_handle,
    )?;
    let total_size = restore_plan.total_size;

    let original_parent = original_path
        .parent()
        .ok_or("无法获取原路径的父目录")?;
    crate::utils::check_disk_space_for_restore(original_parent, total_size)?;

    emit_progress(app_handle, task_id, 9.0, "linking", "正在移除原目录链接...", 0, total_size);
    if crate::utils::is_junction(original_path) {
        fs::remove_dir(original_path)
            .map_err(|e| format!("删除目录链接失败: {}", e))?;
    } else if original_path.exists() {
        return Err(format!(
            "原路径 {} 已存在且不是目录链接，拒绝恢复以保护数据。",
            original_path.display()
        ));
    }

    fs::create_dir_all(original_path)
        .map_err(|e| format!("创建原路径目录失败 {}: {}", original_path.display(), e))?;

    if let Err(e) = copy_dir_with_progress(restore_plan, task_id, &cancel_flag, app_handle) {
        let rollback = rollback_restore_link(original_path, target_path);
        return Err(format!("恢复复制失败：{}\n{}", e, rollback));
    }

    emit_progress(app_handle, task_id, 90.0, "verifying", "正在校验恢复完整性...", total_size, total_size);
    let restored_size = crate::utils::get_dir_size_safe(original_path);
    let tolerance = (total_size as f64 * 0.01) as u64 + 1024 * 1024;
    if (restored_size as i64 - total_size as i64).abs() > tolerance as i64 {
        let rollback = rollback_restore_link(original_path, target_path);
        return Err(format!(
            "恢复完整性校验失败：预期 {}，实际 {}。\n{}",
            format_bytes(total_size),
            format_bytes(restored_size),
            rollback
        ));
    }

    emit_progress(app_handle, task_id, 96.0, "linking", "正在清理目标副本...", restored_size, total_size);
    let cleanup_warning = if target_path.exists() {
        remove_directory_robust(target_path)
            .err()
            .map(|e| format!("恢复已完成，但目标副本清理失败：{}", e))
    } else {
        None
    };

    // 还原成功后清理历史残留备份（迁移时备份清理失败遗留的多余副本），
    // 仍被占用的文件标记为重启后自动删除
    if let Some(parent) = original_path.parent() {
        cleanup_leftover_backups(parent);
    }

    emit_progress(app_handle, task_id, 100.0, "done", "恢复完成", restored_size, total_size);
    Ok(RestoreDirectoryResult { restored_size, cleanup_warning })
}
