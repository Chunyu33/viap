// 重启后删除项的复核
//
// 卸载/清理过程中，被进程占用而删不掉的文件会交给系统在重启时删除
// （MoveFileEx + MOVEFILE_DELAY_UNTIL_REBOOT）。系统是否真的删掉了，用户看不到，
// 因此这里在安排重启删除时记一笔，下次启动时逐项核对：
// - 已经消失 → 清理记录，启动提示里告诉用户"上次安排的重启删除已完成"
// - 仍然存在 → 保留记录（系统也可能失败），在提示里列出，便于用户手动处理
//
// 存储：数据目录下的 pending_reboot_cleanup.json（必须落盘，否则重启后就没有依据了），
// 条目上限 200，防止反复失败时无限增长。

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::data_dir::ensure_data_dir;

const STORAGE_FILE_NAME: &str = "pending_reboot_cleanup.json";
const STORAGE_VERSION: u32 = 1;
/// 条目上限：反复删除失败时不至于让文件无限增长
const MAX_ENTRIES: usize = 200;
/// 启动提示里最多列出的路径数，避免提示过长
const MAX_NOTICE_ITEMS: usize = 5;

/// 已安排重启删除的条目
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PendingRebootEntry {
    /// 待删除路径
    pub path: String,
    /// 所属应用（便于提示与排查）
    pub app_name: String,
    /// 安排时间（Unix 毫秒）
    pub scheduled_at: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct PendingRebootStorage {
    version: u32,
    entries: Vec<PendingRebootEntry>,
}

impl Default for PendingRebootStorage {
    fn default() -> Self {
        Self { version: STORAGE_VERSION, entries: Vec::new() }
    }
}

/// 复核结果
#[derive(Debug, Default, Serialize)]
pub struct RebootVerificationOutcome {
    /// 已被系统删除的路径
    pub removed: Vec<String>,
    /// 重启后依然存在的路径
    pub still_present: Vec<String>,
}

impl RebootVerificationOutcome {
    /// 是否需要提示用户（没有任何条目时返回 None）
    pub fn into_notice(self) -> Option<String> {
        if self.removed.is_empty() && self.still_present.is_empty() {
            return None;
        }

        let mut message = String::new();
        if !self.removed.is_empty() {
            message.push_str(&format!(
                "上次安排的重启删除已完成，共清理 {} 项残留。",
                self.removed.len()
            ));
        }
        if !self.still_present.is_empty() {
            if !message.is_empty() {
                message.push('\n');
            }
            message.push_str(&format!(
                "仍有 {} 项未能删除（可能又被程序占用），可稍后重试：",
                self.still_present.len()
            ));
            for path in self.still_present.iter().take(MAX_NOTICE_ITEMS) {
                message.push_str(&format!("\n· {}", path));
            }
            if self.still_present.len() > MAX_NOTICE_ITEMS {
                message.push_str(&format!(
                    "\n… 其余 {} 项见数据目录中的 {}",
                    self.still_present.len() - MAX_NOTICE_ITEMS,
                    STORAGE_FILE_NAME
                ));
            }
        }
        Some(message)
    }
}

fn storage_path() -> PathBuf {
    ensure_data_dir().join(STORAGE_FILE_NAME)
}


/// 批量记录已安排重启删除的路径
///
/// 一次清理可能安排几十个文件，逐个读改写 JSON 属于无谓 IO，
/// 因此由调用方收集完成后统一记录一次（失败只记日志，不影响删除流程）。
pub fn record_pending_many(paths: &[String], app_name: &str) {
    if paths.is_empty() {
        return;
    }
    if let Err(error) = record_pending_many_at(&storage_path(), paths, app_name) {
        log_warn!("uninstall", "记录重启删除项失败: {}", error);
    }
}

/// 复核所有待删除项：已消失的出队，仍存在的保留
pub fn verify_pending() -> RebootVerificationOutcome {
    verify_pending_at(&storage_path())
}

fn record_pending_many_at(storage: &Path, paths: &[String], app_name: &str) -> Result<(), String> {
    let mut data = load_storage(storage);
    let scheduled_at = now_millis();

    for path in paths {
        // 同一路径重复安排时只保留最新一次
        data.entries.retain(|entry| entry.path != *path);
        data.entries.push(PendingRebootEntry {
            path: path.clone(),
            app_name: app_name.to_string(),
            scheduled_at,
        });
    }
    // 超出上限时丢弃最早的记录
    if data.entries.len() > MAX_ENTRIES {
        let overflow = data.entries.len() - MAX_ENTRIES;
        data.entries.drain(0..overflow);
    }

    write_storage(storage, &data)
}

fn verify_pending_at(storage: &Path) -> RebootVerificationOutcome {
    let mut data = load_storage(storage);
    if data.entries.is_empty() {
        return RebootVerificationOutcome::default();
    }

    let mut outcome = RebootVerificationOutcome::default();
    let mut remaining: Vec<PendingRebootEntry> = Vec::new();
    for entry in data.entries.drain(..) {
        if Path::new(&entry.path).exists() {
            outcome.still_present.push(entry.path.clone());
            remaining.push(entry);
        } else {
            outcome.removed.push(entry.path);
        }
    }

    // 仍存在的条目留到下次启动继续核对；没有任何条目被移除时不必写盘
    if outcome.removed.is_empty() {
        return outcome;
    }
    data.entries = remaining;
    if let Err(error) = write_storage(storage, &data) {
        log_warn!("uninstall", "更新重启删除记录失败: {}", error);
    }
    outcome
}

fn load_storage(storage: &Path) -> PendingRebootStorage {
    let Ok(contents) = fs::read_to_string(storage) else {
        return PendingRebootStorage::default();
    };
    match serde_json::from_str::<PendingRebootStorage>(&contents) {
        Ok(data) => data,
        Err(error) => {
            log_warn!("uninstall", "重启删除记录格式无效，已重置: {}", error);
            PendingRebootStorage::default()
        }
    }
}

/// 原子写入（temp → rename），避免断电留下半个 JSON
fn write_storage(storage: &Path, data: &PendingRebootStorage) -> Result<(), String> {
    let json = serde_json::to_string_pretty(data)
        .map_err(|error| format!("序列化重启删除记录失败: {}", error))?;
    if let Some(parent) = storage.parent() {
        fs::create_dir_all(parent).map_err(|error| format!("创建数据目录失败: {}", error))?;
    }

    let temp_path = storage.with_extension("json.tmp");
    let mut file = fs::File::create(&temp_path).map_err(|error| format!("创建临时文件失败: {}", error))?;
    file.write_all(json.as_bytes()).map_err(|error| format!("写入临时文件失败: {}", error))?;
    file.sync_all().map_err(|error| format!("同步临时文件失败: {}", error))?;
    fs::rename(&temp_path, storage).map_err(|error| format!("替换重启删除记录失败: {}", error))
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_storage(tag: &str) -> PathBuf {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("系统时间应有效")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("viap-reboot-{tag}-{suffix}"));
        fs::create_dir_all(&root).expect("创建测试目录失败");
        root.join(STORAGE_FILE_NAME)
    }

    #[test]
    fn records_entries_without_duplicates_and_caps_the_list() {
        let storage = temp_storage("record");

        record_pending_many_at(
            &storage,
            &[r"C:\missing\a.txt".to_string(), r"C:\missing\a.txt".to_string()],
            "AppA",
        )
        .expect("记录失败");
        record_pending_many_at(&storage, &[r"C:\missing\b.txt".to_string()], "AppB")
            .expect("记录失败");

        let entries = load_storage(&storage).entries;
        assert_eq!(entries.len(), 2, "同一路径重复记录应当去重");
        assert_eq!(entries[0].app_name, "AppA");
        assert_eq!(entries[1].app_name, "AppB");

        // 超过上限时丢弃最早记录
        let overflow_paths: Vec<String> = (0..(MAX_ENTRIES + 5))
            .map(|index| format!(r"C:\missing\f{}.txt", index))
            .collect();
        record_pending_many_at(&storage, &overflow_paths, "AppC").expect("记录失败");
        assert_eq!(load_storage(&storage).entries.len(), MAX_ENTRIES);

        let _ = fs::remove_dir_all(storage.parent().unwrap());
    }

    #[test]
    fn verification_splits_removed_and_still_present() {
        let storage = temp_storage("verify");
        let root = storage.parent().unwrap().to_path_buf();
        // 仍然存在的文件 → 应当留在记录里
        let kept = root.join("kept.txt");
        fs::write(&kept, b"x").expect("写入测试文件失败");
        open_file_handle(&kept);
        record_pending_many_at(
            &storage,
            &[kept.to_string_lossy().to_string(), r"C:\missing\gone.txt".to_string()],
            "AppA",
        )
        .expect("记录失败");

        let outcome = verify_pending_at(&storage);
        assert_eq!(outcome.removed.len(), 1);
        assert_eq!(outcome.still_present.len(), 1);
        assert!(outcome.still_present[0].ends_with("kept.txt"));

        // 仍存在的条目必须留在文件里，下次启动继续核对
        let remaining = load_storage(&storage).entries;
        assert_eq!(remaining.len(), 1);
        assert!(remaining[0].path.ends_with("kept.txt"));

        let notice = outcome.into_notice().expect("应当给出提示");
        assert!(notice.contains("上次安排的重启删除已完成"));
        assert!(notice.contains("仍有 1 项未能删除"));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn empty_storage_produces_no_notice() {
        let storage = temp_storage("empty");
        let outcome = verify_pending_at(&storage);
        assert!(outcome.into_notice().is_none());
        assert!(load_storage(&storage).entries.is_empty());
        let _ = fs::remove_dir_all(storage.parent().unwrap());
    }

    /// 保持文件句柄（模拟"仍被占用"），仅为让测试意图清晰
    fn open_file_handle(path: &Path) {
        let _ = fs::File::open(path);
    }
}
