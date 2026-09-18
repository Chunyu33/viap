// 迁移中断日志（崩溃 / 断电兜底）
//
// 迁移切换是"同卷改名 → 创建链接"两步：改名成功后、链接建好前如果进程崩溃或断电，
// 原路径会消失、数据留在 `.viap_migration_backup_*` 里。此时应用直接不可用，
// 用户也不知道数据去了哪里。
//
// 做法：改名之前先写一条日志，进程启动时据此把备份改回原名，把用户拉回可用状态。
// 恢复只针对日志里记录的精确路径，且只在"原路径缺失 + 备份存在"时动手，
// 任何异常情况都只提示、不删除。

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::data_dir::ensure_data_dir;

const JOURNAL_FILE_NAME: &str = "pending_migration.json";

/// 待确认的迁移切换记录
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingMigration {
    /// 迁移源路径：崩溃后应当重新出现的位置
    pub source_path: String,
    /// 迁移目标位置（用于提示用户数据可能在哪里）
    pub target_path: String,
    /// 切换前创建的临时备份路径
    pub backup_path: String,
}

fn journal_path() -> PathBuf {
    ensure_data_dir().join(JOURNAL_FILE_NAME)
}

/// 日志读写核心：接收日志路径参数，便于单测在临时目录里验证而不碰真实数据目录
fn record_pending_migration_at(journal: &Path, source: &Path, target: &Path, backup: &Path) {
    let pending = PendingMigration {
        source_path: source.to_string_lossy().to_string(),
        target_path: target.to_string_lossy().to_string(),
        backup_path: backup.to_string_lossy().to_string(),
    };
    let Ok(json) = serde_json::to_string_pretty(&pending) else {
        log_warn!("migration", "序列化迁移中断日志失败");
        return;
    };
    if let Err(error) = fs::write(journal, json) {
        log_warn!("migration", "写入迁移中断日志失败: {}", error);
    }
}

fn clear_pending_migration_at(journal: &Path) {
    if journal.exists() {
        if let Err(error) = fs::remove_file(journal) {
            log_warn!("migration", "清除迁移中断日志失败: {}", error);
        }
    }
}

fn load_pending_migration_at(journal: &Path) -> Option<PendingMigration> {
    if !journal.exists() {
        return None;
    }
    match fs::read_to_string(journal) {
        Ok(contents) => match serde_json::from_str::<PendingMigration>(&contents) {
            Ok(pending) => Some(pending),
            Err(error) => {
                log_warn!("migration", "迁移中断日志格式无效，已忽略: {}", error);
                None
            }
        },
        Err(error) => {
            log_warn!("migration", "读取迁移中断日志失败: {}", error);
            None
        }
    }
}

/// 记录"即将开始切换"的迁移
///
/// 写日志失败只意味着少了一层兜底，不应阻塞迁移，因此仅记录日志。
pub fn record_pending_migration(source: &Path, target: &Path, backup: &Path) {
    record_pending_migration_at(&journal_path(), source, target, backup);
}

/// 迁移流程正常结束（成功或已回滚）后清除日志
pub fn clear_pending_migration() {
    clear_pending_migration_at(&journal_path());
}


/// 启动时处理上次中断的迁移，返回需要提示用户的文案（无需提示时为 None）
pub fn recover_interrupted_migration() -> Option<String> {
    recover_interrupted_migration_at(&journal_path())
}

fn recover_interrupted_migration_at(journal: &Path) -> Option<String> {
    let pending = load_pending_migration_at(journal)?;
    let source = PathBuf::from(&pending.source_path);
    let backup = PathBuf::from(&pending.backup_path);

    // 原路径已经存在：切换已完成（或改名根本没发生），顺手清掉可能残留的备份
    if source.exists() {
        if backup.exists() {
            let _ = crate::migration::remove_directory_robust(&backup);
        }
        clear_pending_migration_at(journal);
        return None;
    }

    // 原路径消失且备份存在：正是"改名后崩溃"，把备份改回原名即可恢复可用
    if backup.exists() {
        return match fs::rename(&backup, &source) {
            Ok(()) => {
                clear_pending_migration_at(journal);
                Some(format!(
                    "上次迁移被中断，已自动把「{}」还原回 {}。\n\
                     目标位置可能残留一份未完成的副本：{}，确认数据无误后可手动清理。",
                    pending.source_path, pending.source_path, pending.target_path
                ))
            }
            Err(error) => {
                log_warn!(
                    "migration",
                    "自动还原中断的迁移失败 {} -> {}: {}",
                    backup.display(),
                    source.display(),
                    error
                );
                // 保留日志，下次启动再试；同时提示用户数据位置
                Some(format!(
                    "上次迁移被中断，自动还原未成功。\n\
                     数据仍在：{}\n原路径：{}",
                    pending.backup_path, pending.source_path
                ))
            }
        };
    }

    // 原路径与备份都不存在：数据可能已复制到目标位置，只提示不删除
    clear_pending_migration_at(journal);
    Some(format!(
        "上次迁移被中断，原路径 {} 不存在，也没有找到切换前的临时备份。\n\
         数据可能仍完整保存在：{}",
        pending.source_path, pending.target_path
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn create_fixture(tag: &str) -> (PathBuf, PathBuf, PathBuf) {
        let suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("系统时间应有效")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("viap-pending-{tag}-{suffix}"));
        let source = root.join("source");
        let backup = root.join(format!(".viap_migration_backup_{tag}"));
        let target = root.join("target");
        std::fs::create_dir_all(&root).expect("创建测试根目录失败");
        (source, backup, target)
    }

    /// 改名后崩溃：备份被改回原名，用户重新可用
    #[test]
    fn restores_backup_when_source_disappeared() {
        let (source, backup, target) = create_fixture("restore");
        std::fs::create_dir_all(&backup).expect("创建备份目录失败");
        std::fs::write(backup.join("data.bin"), b"payload").expect("写入备份内容失败");
        std::fs::create_dir_all(&target).expect("创建目标目录失败");

        let journal = source.parent().unwrap().join("journal.json");
        record_pending_migration_at(&journal, &source, &target, &backup);
        assert!(load_pending_migration_at(&journal).is_some());

        let notice = recover_interrupted_migration_at(&journal);
        assert!(notice.is_some(), "应当提示用户已自动还原");
        assert!(source.join("data.bin").is_file(), "备份内容必须回到原路径");
        assert!(!backup.exists(), "备份目录应当已被改名");
        assert!(load_pending_migration_at(&journal).is_none(), "恢复后日志必须清除");

        std::fs::remove_dir_all(source.parent().unwrap()).ok();
    }

    /// 切换已完成：原路径存在时不做任何移动，只清理日志
    #[test]
    fn keeps_everything_when_source_still_exists() {
        let (source, backup, target) = create_fixture("done");
        std::fs::create_dir_all(&source).expect("创建源目录失败");
        std::fs::write(source.join("data.bin"), b"payload").expect("写入源内容失败");
        std::fs::create_dir_all(&target).expect("创建目标目录失败");

        let journal = source.parent().unwrap().join("journal.json");
        record_pending_migration_at(&journal, &source, &target, &backup);
        let notice = recover_interrupted_migration_at(&journal);

        assert!(notice.is_none(), "切换已完成时不需要提示");
        assert!(source.join("data.bin").is_file(), "原有数据不能被动过");
        assert!(load_pending_migration_at(&journal).is_none());

        std::fs::remove_dir_all(source.parent().unwrap()).ok();
    }

    /// 源与备份都不在：只提示数据可能在目标位置，不做删除
    #[test]
    fn reports_missing_backup_without_deleting_anything() {
        let (source, backup, target) = create_fixture("missing");
        std::fs::create_dir_all(&target).expect("创建目标目录失败");
        std::fs::write(target.join("copied.bin"), b"copied").expect("写入目标内容失败");

        let journal = target.parent().unwrap().join("journal.json");
        record_pending_migration_at(&journal, &source, &target, &backup);
        let notice = recover_interrupted_migration_at(&journal).expect("应当提示数据位置");

        assert!(notice.contains(&target.to_string_lossy().to_string()));
        assert!(target.join("copied.bin").is_file(), "不能删除目标位置的任何数据");
        assert!(load_pending_migration_at(&journal).is_none());

        std::fs::remove_dir_all(target.parent().unwrap()).ok();
    }
}
