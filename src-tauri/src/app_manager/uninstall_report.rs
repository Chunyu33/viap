// 卸载报告持久化（用户主动保存时才写盘）
//
// 报告 = 前端的可读字段 + 后端内存里的快照与差异 + 版本号。
// 保存位置在数据目录的 uninstall_reports/ 下，文件名形如
// `<应用名>-<毫秒时间戳>.json`，轮转保留最近 MAX_KEPT_REPORTS 份。

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use super::uninstall_snapshot::{self, UninstallSnapshot, UninstallSnapshotDiff};
use crate::storage::data_dir::ensure_data_dir;


/// 轮转保留的报告份数
const MAX_KEPT_REPORTS: usize = 20;
/// 文件名里应用名部分的长度上限
const MAX_SLUG_LENGTH: usize = 40;

/// 保存报告的入参（前端已知的字段）
#[derive(Debug, Deserialize)]
pub struct UninstallReportInput {
    pub app_name: String,
    pub install_location: String,
    pub estimated_bytes: u64,
    pub uninstall_freed_bytes: u64,
    pub cleanup_freed_bytes: u64,
    #[serde(default)]
    pub failed_items: Vec<String>,
    #[serde(default)]
    pub scheduled_for_reboot: Vec<String>,
}

/// 落盘的报告内容
#[derive(Debug, Serialize, Deserialize)]
pub struct SavedUninstallReport {
    pub version: u32,
    pub created_at: u64,
    pub app_name: String,
    pub install_location: String,
    pub estimated_bytes: u64,
    pub uninstall_freed_bytes: u64,
    pub cleanup_freed_bytes: u64,
    pub failed_items: Vec<String>,
    pub scheduled_for_reboot: Vec<String>,
    /// 卸载前快照；内存里已被新一轮动作覆盖时为 null
    pub snapshot: Option<UninstallSnapshot>,
    /// 卸载前后差异
    pub diff: Option<UninstallSnapshotDiff>,
}

/// 保存结果（前端用于提示）
#[derive(Debug, Serialize)]
pub struct SaveReportOutcome {
    pub path: String,
    /// 轮转时清理掉的旧报告数量
    pub pruned_count: usize,
}

/// 报告目录：数据目录下的 uninstall_reports/
pub(crate) fn reports_dir() -> PathBuf {
    ensure_data_dir().join(crate::storage::data_dir::UNINSTALL_REPORTS_DIR_NAME)
}

/// 保存卸载报告并轮转旧报告
#[tauri::command]
pub fn save_uninstall_report(input: UninstallReportInput) -> Result<SaveReportOutcome, String> {
    let dir = reports_dir();
    let created_at = now_millis();

    let (snapshot, diff) = match uninstall_snapshot::current_snapshot() {
        Some(snapshot) if snapshot.app_name == input.app_name => {
            let diff = uninstall_snapshot::current_diff();
            (Some(snapshot), diff)
        }
        _ => (None, None),
    };

    let report = SavedUninstallReport {
        version: 1,
        created_at,
        app_name: input.app_name.clone(),
        install_location: input.install_location,
        estimated_bytes: input.estimated_bytes,
        uninstall_freed_bytes: input.uninstall_freed_bytes,
        cleanup_freed_bytes: input.cleanup_freed_bytes,
        failed_items: input.failed_items,
        scheduled_for_reboot: input.scheduled_for_reboot,
        // 快照与差异只存在于内存。内存里可能已经是"另一个应用"的快照
        // （例如用户连续卸载两个应用），名字对不上时不写入，避免报告张冠李戴。
        snapshot,
        diff,
    };

    let file_name = format!("{}-{}.json", sanitize_report_slug(&input.app_name), created_at);
    let path = dir.join(file_name);
    write_report_at(&path, &report)?;
    let pruned_count = prune_reports_at(&dir, MAX_KEPT_REPORTS)?;

    Ok(SaveReportOutcome {
        path: path.to_string_lossy().to_string(),
        pruned_count,
    })
}

/// 生成安全的文件名片段：只保留中英文、数字与常见符号，其余替换为下划线
///
/// 注意按**字符**而非字节截断：中文应用名在 40 字节处可能落在字符中间，
/// `String::truncate` 在这种情况下会 panic。
pub(crate) fn sanitize_report_slug(app_name: &str) -> String {
    let sanitized: String = app_name
        .trim()
        .chars()
        .map(|ch| {
            if ch.is_alphanumeric() || matches!(ch, '.' | '_' | '-') {
                ch
            } else {
                '_'
            }
        })
        .collect();

    // 去掉首尾的下划线与点，避免产生隐藏文件或空名
    let trimmed = sanitized.trim_matches(|ch| ch == '_' || ch == '.');
    let slug: String = trimmed.chars().take(MAX_SLUG_LENGTH).collect();
    if slug.is_empty() {
        return "app".to_string();
    }
    slug
}

fn write_report_at(path: &Path, report: &SavedUninstallReport) -> Result<(), String> {
    let json = serde_json::to_string_pretty(report)
        .map_err(|error| format!("序列化卸载报告失败: {}", error))?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| format!("创建报告目录失败: {}", error))?;
    }
    fs::write(path, json).map_err(|error| format!("写入卸载报告失败: {}", error))
}

/// 轮转：只保留最新的 `keep` 份报告（按修改时间），返回删除数量
pub(crate) fn prune_reports_at(dir: &Path, keep: usize) -> Result<usize, String> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Ok(0);
    };

    let mut reports: Vec<(SystemTime, PathBuf)> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let is_report = path
            .extension()
            .and_then(|extension| extension.to_str())
            .map(|extension| extension.eq_ignore_ascii_case("json"))
            .unwrap_or(false);
        if !is_report || !path.is_file() {
            continue;
        }
        let modified = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .unwrap_or(UNIX_EPOCH);
        reports.push((modified, path));
    }

    if reports.len() <= keep {
        return Ok(0);
    }

    reports.sort_by(|left, right| right.0.cmp(&left.0));
    let mut pruned = 0usize;
    for (_, path) in reports.into_iter().skip(keep) {
        match fs::remove_file(&path) {
            Ok(()) => pruned += 1,
            // 单个文件删不掉不影响保存结果，只记录
            Err(error) => log_warn!("uninstall", "清理旧报告失败 {}: {}", path.display(), error),
        }
    }
    Ok(pruned)
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

    fn temp_dir(tag: &str) -> PathBuf {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("系统时间应有效")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("viap-report-{tag}-{suffix}"));
        fs::create_dir_all(&dir).expect("创建测试目录失败");
        dir
    }

    fn sample_report(name: &str) -> SavedUninstallReport {
        SavedUninstallReport {
            version: 1,
            created_at: now_millis(),
            app_name: name.to_string(),
            install_location: r"D:\apps\Sample".to_string(),
            estimated_bytes: 1024,
            uninstall_freed_bytes: 512,
            cleanup_freed_bytes: 256,
            failed_items: vec![r"D:\apps\Sample\locked.dll".to_string()],
            scheduled_for_reboot: vec![r"D:\apps\Sample\locked.dll".to_string()],
            snapshot: None,
            diff: None,
        }
    }

    #[test]
    fn slug_is_filesystem_safe() {
        assert_eq!(sanitize_report_slug("Clash Verge"), "Clash_Verge");
        assert_eq!(sanitize_report_slug("微信 WeChat"), "微信_WeChat");
        // 路径分隔符与通配符必须被替换掉，避免越界写文件
        assert_eq!(sanitize_report_slug(r"..\..\Windows\System32"), "Windows_System32");
        assert_eq!(sanitize_report_slug("***"), "app");
        assert_eq!(sanitize_report_slug(""), "app");
        // 超长名字按字符截断（不能按字节，否则中文名会落在字符中间导致 panic）
        assert_eq!(sanitize_report_slug(&"a".repeat(100)).chars().count(), MAX_SLUG_LENGTH);
        let chinese_name = "微信".repeat(40);
        let slug = sanitize_report_slug(&chinese_name);
        assert_eq!(slug.chars().count(), MAX_SLUG_LENGTH);
        assert!(slug.chars().all(|ch| ch == '微' || ch == '信'));
    }

    #[test]
    fn prune_keeps_only_the_newest_reports() {
        let dir = temp_dir("prune");
        for index in 0..5 {
            write_report_at(&dir.join(format!("app-{}.json", index)), &sample_report("app"))
                .expect("写入报告失败");
            // 保证修改时间有差异，避免同毫秒导致排序不确定
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        // 非报告文件不应被轮转删除
        let other = dir.join("keep.txt");
        fs::write(&other, b"x").expect("写入测试文件失败");

        let pruned = prune_reports_at(&dir, 2).expect("轮转失败");
        assert_eq!(pruned, 3);
        let remaining: Vec<String> = fs::read_dir(&dir)
            .expect("读取目录失败")
            .flatten()
            .map(|entry| entry.file_name().to_string_lossy().to_string())
            .collect();
        assert_eq!(remaining.iter().filter(|name| name.ends_with(".json")).count(), 2);
        assert!(other.exists(), "非报告文件不能被删除");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn saved_report_round_trips_with_all_fields() {
        let dir = temp_dir("roundtrip");
        let path = dir.join("app-1.json");
        let report = sample_report("Sample App");
        write_report_at(&path, &report).expect("写入报告失败");

        let contents = fs::read_to_string(&path).expect("读取报告失败");
        let parsed: SavedUninstallReport = serde_json::from_str(&contents).expect("解析报告失败");
        assert_eq!(parsed.app_name, "Sample App");
        assert_eq!(parsed.estimated_bytes, 1024);
        assert_eq!(parsed.uninstall_freed_bytes + parsed.cleanup_freed_bytes, 768);
        assert_eq!(parsed.failed_items.len(), 1);
        assert!(parsed.snapshot.is_none(), "未采集快照时不应写入快照");

        let _ = fs::remove_dir_all(&dir);
    }
}
