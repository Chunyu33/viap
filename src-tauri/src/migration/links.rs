// 目录链接子模块
// Junction / 软链接的创建、验证、预检、临时备份路径与回滚

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::migration::cleanup::remove_directory_robust;

#[cfg(windows)]
use std::os::windows::fs::symlink_dir;

pub(crate) fn create_directory_link(target: &Path, link: &Path) -> Result<&'static str, String> {
    // Junction 不依赖开发者模式/管理员软链接权限，优先尝试可降低跨盘迁移失败率。
    match junction::create(target, link) {
        Ok(_) => return Ok("Junction"),
        Err(e) => {
            log_warn!("migration", "Junction 创建失败，降级软链接: {}", e);
        }
    }

    // Junction 降级：尝试软链接，兼容少数 Junction 不可用的文件系统或路径形态。
    match symlink_dir(target, link) {
        Ok(_) => Ok("Symlink"),
        Err(e) => {
            let os_err = e.raw_os_error().unwrap_or(0);
            let reason = if os_err == 1314 || os_err == 5 {
                format!(
                    "创建目录链接失败：权限不足（错误码 {}）。\n\n\
                     Junction 和软链接均失败，请以管理员身份运行本程序后重试。",
                    os_err
                )
            } else {
                format!(
                    "创建目录链接失败（错误码 {}）：{}\n\n\
                     请以管理员身份运行本程序后重试。",
                    os_err, e
                )
            };
            Err(reason)
        }
    }
}

/// 删除临时目录链接只用 remove_dir，避免 remove_dir_all 误递归到链接目标。
#[cfg(windows)]
pub(crate) fn remove_directory_link(link: &Path) -> Result<(), String> {
    fs::remove_dir(link)
        .map_err(|e| format!("清理临时目录链接失败 {}: {}", link.display(), e))
}

/// 在删除源目录前先用临时名称验证链接能力，失败时源目录仍完整保留。
#[cfg(windows)]
pub(crate) fn preflight_directory_link(target: &Path, source: &Path) -> Result<(), String> {
    let parent = source.parent()
        .ok_or_else(|| format!("源目录缺少父目录，无法预检链接: {}", source.display()))?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    let probe_name = format!(".viap_link_probe_{}_{}", std::process::id(), timestamp);
    let probe_link = parent.join(probe_name);

    if probe_link.exists() || probe_link.is_symlink() {
        return Err(format!("临时链接路径已存在: {}", probe_link.display()));
    }

    // 先验证同一父目录下可创建链接；若失败直接中止，不删除源目录。
    create_directory_link(target, &probe_link)?;
    remove_directory_link(&probe_link)
}

/// 为源目录生成同父目录下的临时备份路径。
///
/// 迁移切换必须使用同卷 rename，临时目录放在源目录父级可以保证改名是原子的，
/// 同时避免把正在迁移的目录再次纳入目标路径扫描。
#[cfg(windows)]
pub(crate) fn create_migration_backup_path(source: &Path) -> Result<PathBuf, String> {
    let parent = source
        .parent()
        .ok_or_else(|| format!("源目录缺少父目录，无法创建迁移备份：{}", source.display()))?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);

    for attempt in 0..10 {
        let backup_name = format!(
            ".viap_migration_backup_{}_{}_{}",
            std::process::id(),
            timestamp,
            attempt
        );
        let backup_path = parent.join(backup_name);

        // symlink_metadata 能识别悬空重解析点，避免临时路径碰撞后误改用户目录。
        if backup_path.symlink_metadata().is_err() {
            return Ok(backup_path);
        }
    }

    Err(format!(
        "无法创建唯一的迁移备份路径，请清理 {} 下的 .viap_migration_backup_* 后重试",
        parent.display()
    ))
}

/// 校验原路径已经是指向预期目标的目录链接。
///
/// create_directory_link 返回成功后仍做一次实际路径确认，防止链接创建过程被外部程序
/// 干扰时误进入清理备份阶段。
#[cfg(windows)]
pub(crate) fn verify_directory_link(link: &Path, target: &Path) -> Result<(), String> {
    if !crate::utils::is_junction(link) {
        return Err(format!("原路径未成为目录链接：{}", link.display()));
    }

    let actual_target = crate::utils::get_junction_target(link)
        .ok_or_else(|| format!("无法读取目录链接目标：{}", link.display()))?;
    let expected = fs::canonicalize(target)
        .map_err(|e| format!("无法解析目标目录：{}，原因：{}", target.display(), e))?;
    let actual = fs::canonicalize(Path::new(&actual_target))
        .map_err(|e| format!("无法解析目录链接目标：{}，原因：{}", actual_target, e))?;

    if actual != expected {
        return Err(format!(
            "目录链接目标不一致：预期 {}，实际 {}",
            expected.display(),
            actual.display()
        ));
    }

    Ok(())
}

/// 在链接创建失败时，只移除 Viap 创建的目录链接并恢复原目录。
///
/// 对非链接目录绝不递归删除，避免异常状态下把用户新建的数据目录当成半成品清理。
#[cfg(windows)]
pub(crate) fn restore_source_from_backup(source: &Path, backup: &Path, target: &Path) -> Result<(), String> {
    if crate::utils::is_junction(source) {
        verify_directory_link(source, target)?;
        remove_directory_link(source)?;
    } else if source.symlink_metadata().is_ok() {
        return Err(format!(
            "原路径出现非目录链接内容，拒绝覆盖以保护数据：{}",
            source.display()
        ));
    }

    fs::rename(backup, source).map_err(|e| {
        format!(
            "恢复原目录失败：{} -> {}，原因：{}",
            backup.display(),
            source.display(),
            e
        )
    })
}

#[cfg(windows)]
pub(crate) fn rollback_restore_link(original_path: &Path, target_path: &Path) -> String {
    let cleanup_result = if original_path.exists() && !crate::utils::is_junction(original_path) {
        remove_directory_robust(original_path)
            .map_err(|e| format!("清理原路径半成品失败: {}", e))
    } else {
        Ok(())
    };

    let link_result = if !original_path.exists() {
        create_directory_link(target_path, original_path)
            .map(|_| ())
            .map_err(|e| format!("重建目录链接失败: {}", e))
    } else {
        Ok(())
    };

    match (cleanup_result, link_result) {
        (Ok(_), Ok(_)) => format!(
            "已自动恢复目录链接，数据仍完整保存在：{}",
            target_path.display()
        ),
        (cleanup, link) => format!(
            "自动回滚未完全成功。目标数据仍完整保存在：{}；cleanup={:?}；link={:?}",
            target_path.display(), cleanup.err(), link.err()
        ),
    }
}

