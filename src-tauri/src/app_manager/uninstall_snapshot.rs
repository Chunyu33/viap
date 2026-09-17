// 卸载前后快照与对比（只存内存，默认不落盘）
//
// 为什么要它：残留扫描靠"名称匹配"，而现实中残留常叫发布商名、GUID、缩写
// （AppData\Roaming\Tencent、AppData\Local\{GUID}），名字对不上就会漏。
// 快照对比不依赖名字：卸载前存在、卸载后消失/仍在，一目了然。
//
// 数据边界（这是性能与体积的关键）：
// - 只记录"标准位置的顶层子项名"，不做递归清单 —— 实测深度 5 的递归在单机
//   LOCALAPPDATA 就有 29.7 万条目、耗时 30 秒，ProgramData 还会一路撞权限拒绝
// - 顶层清单实测共 333 条、约 1 ms；加上 Uninstall 键枚举（332 个、70 ms），
//   整个快照约 80 ms、内存里 20~30 KB
//
// 存储：默认只放在进程内存（静态变量），流程结束做一次 diff 即可；
// 用户主动"保存报告"时才会写文件，避免为一次对比留下成堆 JSON。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ};
use winreg::RegKey;

use super::uninstaller;

/// 单个位置允许记录的最大条目数，避免异常目录把快照撑大
const MAX_ENTRIES_PER_GROUP: usize = 2_000;
/// 位置组数量上限（标准位置 + 安装目录），用于兜底
const MAX_LOCATIONS: usize = 8;

/// 顶层清单里需要忽略的易变目录
///
/// 这些目录由系统或其它程序高频改动，若不排除会灌满"卸载期间新增"列表，
/// 把真正的线索淹没。
const VOLATILE_ENTRY_NAMES: &[&str] = &[
    "temp",
    "tmp",
    "microsoft",
    "packages",
    "crashdumps",
    "d3dscache",
    "connecteddevicesplatform",
    "elevateddiagnostics",
];

/// 一个位置的顶层清单
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SnapshotLocation {
    /// 展示名（如 AppData\Roaming）
    pub label: String,
    /// 实际路径
    pub path: String,
    /// 顶层子项名（保持原始大小写，比较时忽略大小写）
    pub entries: Vec<String>,
}

/// 卸载前快照（仅内存）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UninstallSnapshot {
    pub app_name: String,
    pub install_location: String,
    /// 快照时间（Unix 毫秒）
    pub created_at: u64,
    /// 标准位置与安装目录的顶层清单
    pub locations: Vec<SnapshotLocation>,
    /// 应用自身注册表键及其直接子键（形如 HKCU\Software\Foo、HKCU\Software\Foo\Bar）
    pub registry_keys: Vec<String>,
    /// 卸载登记项名称（Uninstall 键下的子键名）
    pub uninstall_entries: Vec<String>,
}

/// 快照摘要（返回给前端展示"已记录多少项"）
#[derive(Debug, Serialize)]
pub struct SnapshotSummary {
    pub created_at: u64,
    pub location_count: usize,
    pub entry_count: usize,
    pub uninstall_entry_count: usize,
}

/// 差异条目
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SnapshotDiffEntry {
    /// 所属位置（展示名）
    pub group: String,
    /// 名称
    pub name: String,
    /// 归属可信度：certain（安装目录 / 应用注册表键）| uncertain（公共目录顶层项）
    pub confidence: String,
}

/// 快照差异
#[derive(Debug, Serialize)]
pub struct UninstallSnapshotDiff {
    /// 是否存在可用快照
    pub has_snapshot: bool,
    pub created_at: u64,
    /// 卸载期间新出现（可能是卸载器留下的残留，也可能是用户自己新建的）
    pub appeared: Vec<SnapshotDiffEntry>,
    /// 卸载后消失（正常结果）
    pub disappeared: Vec<SnapshotDiffEntry>,
    /// 卸载前后都在（可能是历史残留，也可能属于别的应用）
    pub remaining: Vec<SnapshotDiffEntry>,
}

/// 进程内快照缓存：默认不落盘，流程结束即失去意义
static UNINSTALL_SNAPSHOT: Mutex<Option<UninstallSnapshot>> = Mutex::new(None);

/// 采集卸载前快照并缓存
#[tauri::command]
pub async fn begin_uninstall_snapshot(
    app_name: String,
    install_location: Option<String>,
    registry_path: Option<String>,
) -> Result<SnapshotSummary, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let snapshot = collect_snapshot(
            &app_name,
            install_location.as_deref(),
            registry_path.as_deref(),
        );
        let summary = SnapshotSummary {
            created_at: snapshot.created_at,
            location_count: snapshot.locations.len(),
            entry_count: snapshot.locations.iter().map(|location| location.entries.len()).sum(),
            uninstall_entry_count: snapshot.uninstall_entries.len(),
        };
        if let Ok(mut slot) = UNINSTALL_SNAPSHOT.lock() {
            *slot = Some(snapshot);
        }
        summary
    })
    .await
    .map_err(|error| format!("采集卸载快照失败: {}", error))
}

/// 用当前状态与缓存快照对比
#[tauri::command]
pub async fn diff_uninstall_snapshot() -> Result<UninstallSnapshotDiff, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let snapshot = match UNINSTALL_SNAPSHOT.lock() {
            Ok(slot) => slot.clone(),
            Err(_) => None,
        };
        let Some(snapshot) = snapshot else {
            return UninstallSnapshotDiff {
                has_snapshot: false,
                created_at: 0,
                appeared: Vec::new(),
                disappeared: Vec::new(),
                remaining: Vec::new(),
            };
        };
        compute_diff(&snapshot)
    })
    .await
    .map_err(|error| format!("对比卸载快照失败: {}", error))
}

/// 采集快照：标准位置顶层清单 + 安装目录顶层清单 + 应用注册表键 + 卸载登记项
pub(crate) fn collect_snapshot(
    app_name: &str,
    install_location: Option<&str>,
    registry_path: Option<&str>,
) -> UninstallSnapshot {
    let mut locations: Vec<SnapshotLocation> = Vec::new();

    for (label, path) in standard_locations() {
        if locations.len() >= MAX_LOCATIONS {
            break;
        }
        if !path.is_dir() {
            continue;
        }
        locations.push(SnapshotLocation {
            label: label.to_string(),
            path: path.to_string_lossy().to_string(),
            entries: list_top_level_entries(&path),
        });
    }

    let install_location = install_location
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_default();
    if !install_location.is_empty() {
        let path = PathBuf::from(&install_location);
        if path.is_dir() {
            locations.push(SnapshotLocation {
                label: "安装目录".to_string(),
                path: install_location.clone(),
                entries: list_top_level_entries(&path),
            });
        }
    }

    UninstallSnapshot {
        app_name: app_name.to_string(),
        install_location,
        created_at: now_millis(),
        locations,
        registry_keys: collect_registry_keys(registry_path),
        uninstall_entries: collect_uninstall_entry_names(),
    }
}

/// 需要对比的标准位置（顶层，不递归）
fn standard_locations() -> Vec<(&'static str, PathBuf)> {
    let mut locations: Vec<(&'static str, PathBuf)> = Vec::new();

    if let Ok(appdata) = std::env::var("APPDATA") {
        locations.push(("AppData\\Roaming", PathBuf::from(&appdata)));
        locations.push((
            "开始菜单程序",
            PathBuf::from(&appdata).join(r"Microsoft\Windows\Start Menu\Programs"),
        ));
    }
    if let Ok(local_appdata) = std::env::var("LOCALAPPDATA") {
        locations.push(("AppData\\Local", PathBuf::from(local_appdata)));
    }
    if let Ok(profile) = std::env::var("USERPROFILE") {
        locations.push(("AppData\\LocalLow", PathBuf::from(profile).join(r"AppData\LocalLow")));
    }
    locations.push(("ProgramData", PathBuf::from(r"C:\ProgramData")));

    locations
}

/// 列出一个目录的顶层子项名（忽略易变目录，按名称排序保证稳定）
pub(crate) fn list_top_level_entries(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };

    let mut names: BTreeSet<String> = BTreeSet::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.is_empty() || VOLATILE_ENTRY_NAMES.contains(&name.to_lowercase().as_str()) {
            continue;
        }
        names.insert(name);
        if names.len() >= MAX_ENTRIES_PER_GROUP {
            break;
        }
    }
    names.into_iter().collect()
}

/// 采集应用自身注册表键：键本身 + 直接子键
fn collect_registry_keys(registry_path: Option<&str>) -> Vec<String> {
    let Some(registry_path) = registry_path.map(str::trim).filter(|value| !value.is_empty()) else {
        return Vec::new();
    };
    let Some((hive, sub_path)) = uninstaller::parse_registry_path(registry_path) else {
        return Vec::new();
    };
    let Ok(key) = RegKey::predef(hive).open_subkey_with_flags(sub_path, KEY_READ) else {
        return Vec::new();
    };

    let hive_label = if hive == HKEY_CURRENT_USER { "HKCU" } else { "HKLM" };
    let mut keys: Vec<String> = vec![format!("{}\\{}", hive_label, sub_path)];
    for child in key.enum_keys().filter_map(|name| name.ok()).take(MAX_ENTRIES_PER_GROUP) {
        keys.push(format!("{}\\{}\\{}", hive_label, sub_path, child));
    }
    keys
}

/// 采集当前所有卸载登记项名称（Uninstall 键下的子键名）
fn collect_uninstall_entry_names() -> Vec<String> {
    let roots: [(winreg::HKEY, &str); 4] = [
        (HKEY_LOCAL_MACHINE, r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall"),
        (HKEY_LOCAL_MACHINE, r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall"),
        (HKEY_CURRENT_USER, r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall"),
        (HKEY_CURRENT_USER, r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall"),
    ];

    let mut names: BTreeSet<String> = BTreeSet::new();
    for (hive, path) in roots {
        let Ok(key) = RegKey::predef(hive).open_subkey_with_flags(path, KEY_READ) else {
            continue;
        };
        for name in key.enum_keys().filter_map(|name| name.ok()) {
            names.insert(name);
            if names.len() >= MAX_ENTRIES_PER_GROUP {
                return names.into_iter().collect();
            }
        }
    }
    names.into_iter().collect()
}

/// 计算快照与当前状态的差异
pub(crate) fn compute_diff(snapshot: &UninstallSnapshot) -> UninstallSnapshotDiff {
    let mut appeared: Vec<SnapshotDiffEntry> = Vec::new();
    let mut disappeared: Vec<SnapshotDiffEntry> = Vec::new();
    let mut remaining: Vec<SnapshotDiffEntry> = Vec::new();

    // 各位置的顶层清单
    let current_locations: Vec<(String, Vec<String>)> = standard_locations()
        .into_iter()
        .filter(|(_, path)| path.is_dir())
        .map(|(label, path)| {
            (
                label.to_string(),
                list_top_level_entries(&path),
            )
        })
        .collect();

    for location in &snapshot.locations {
        let is_install_dir = location.label == "安装目录";
        let current: Vec<String> = if is_install_dir {
            list_top_level_entries(Path::new(&location.path))
        } else {
            current_locations
                .iter()
                .find(|(label, _)| label == &location.label)
                .map(|(_, entries)| entries.clone())
                .unwrap_or_default()
        };

        // 安装目录与应用自身注册表键归"归属明确"，公共目录顶层项归"需人工确认"
        let confidence = if is_install_dir { "certain" } else { "uncertain" };
        let (group_appeared, group_disappeared, group_remaining) =
            diff_names(&location.entries, &current);
        appeared.extend(to_entries(&location.label, group_appeared, confidence));
        disappeared.extend(to_entries(&location.label, group_disappeared, confidence));
        remaining.extend(to_entries(&location.label, group_remaining, confidence));
    }

    // 应用注册表键
    let current_registry = collect_registry_keys(Some(&snapshot.registry_keys.first().cloned().unwrap_or_default()));
    if !snapshot.registry_keys.is_empty() {
        let (group_appeared, group_disappeared, group_remaining) =
            diff_names(&snapshot.registry_keys, &current_registry);
        appeared.extend(to_entries("应用注册表键", group_appeared, "certain"));
        disappeared.extend(to_entries("应用注册表键", group_disappeared, "certain"));
        remaining.extend(to_entries("应用注册表键", group_remaining, "certain"));
    }

    // 卸载登记项
    let current_uninstall = collect_uninstall_entry_names();
    let (group_appeared, group_disappeared, group_remaining) =
        diff_names(&snapshot.uninstall_entries, &current_uninstall);
    appeared.extend(to_entries("卸载登记项", group_appeared, "uncertain"));
    disappeared.extend(to_entries("卸载登记项", group_disappeared, "uncertain"));
    remaining.extend(to_entries("卸载登记项", group_remaining, "uncertain"));

    UninstallSnapshotDiff {
        has_snapshot: true,
        created_at: snapshot.created_at,
        appeared,
        disappeared,
        remaining,
    }
}

/// 按名称（忽略大小写）比较两组集合
fn diff_names(before: &[String], after: &[String]) -> (Vec<String>, Vec<String>, Vec<String>) {
    let before_set: BTreeSet<String> = before.iter().map(|name| name.to_lowercase()).collect();
    let after_set: BTreeSet<String> = after.iter().map(|name| name.to_lowercase()).collect();

    let appeared = after
        .iter()
        .filter(|name| !before_set.contains(&name.to_lowercase()))
        .cloned()
        .collect();
    let disappeared = before
        .iter()
        .filter(|name| !after_set.contains(&name.to_lowercase()))
        .cloned()
        .collect();
    let remaining = before
        .iter()
        .filter(|name| after_set.contains(&name.to_lowercase()))
        .cloned()
        .collect();

    (appeared, disappeared, remaining)
}

fn to_entries(group: &str, names: Vec<String>, confidence: &str) -> Vec<SnapshotDiffEntry> {
    names
        .into_iter()
        .map(|name| SnapshotDiffEntry {
            group: group.to_string(),
            name,
            confidence: confidence.to_string(),
        })
        .collect()
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    #[test]
    fn diff_names_ignores_case_and_classifies_three_ways() {
        let before = vec!["Clash Verge".to_string(), "OldApp".to_string(), "Keep".to_string()];
        let after = vec!["clash verge".to_string(), "Keep".to_string(), "NewApp".to_string()];

        let (appeared, disappeared, remaining) = diff_names(&before, &after);
        assert_eq!(appeared, vec!["NewApp".to_string()]);
        assert_eq!(disappeared, vec!["OldApp".to_string()]);
        // 大小写不同视为同一个条目
        assert_eq!(remaining, vec!["Clash Verge".to_string(), "Keep".to_string()]);
    }

    #[test]
    fn top_level_listing_skips_volatile_directories() {
        let suffix = std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("系统时间应有效")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("viap-snapshot-{suffix}"));
        std::fs::create_dir_all(root.join("RealApp")).expect("创建目录失败");
        std::fs::create_dir_all(root.join("Temp")).expect("创建目录失败");
        std::fs::create_dir_all(root.join("D3DSCache")).expect("创建目录失败");

        let entries = list_top_level_entries(&root);
        assert_eq!(entries, vec!["RealApp".to_string()]);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn snapshot_captures_install_dir_and_diff_detects_removal() {
        let suffix = std::time::SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("系统时间应有效")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("viap-snapshot-diff-{suffix}"));
        let install = root.join("SomeApp");
        std::fs::create_dir_all(install.join("bin")).expect("创建安装目录失败");
        std::fs::write(install.join("SomeApp.exe"), b"x").expect("写入文件失败");

        let snapshot = collect_snapshot("SomeApp", Some(&install.to_string_lossy()), None);
        let install_group = snapshot
            .locations
            .iter()
            .find(|location| location.label == "安装目录")
            .expect("快照里应当包含安装目录");
        assert!(install_group.entries.contains(&"SomeApp.exe".to_string()));
        assert!(install_group.entries.contains(&"bin".to_string()));

        // 删掉一个文件后重新对比：应当出现在"消失"里，且归属明确
        std::fs::remove_file(install.join("SomeApp.exe")).expect("删除文件失败");
        let diff = compute_diff(&snapshot);
        assert!(diff.has_snapshot);
        let removed = diff
            .disappeared
            .iter()
            .find(|entry| entry.name == "SomeApp.exe")
            .expect("删除的文件应当出现在差异里");
        assert_eq!(removed.group, "安装目录");
        assert_eq!(removed.confidence, "certain");

        let _ = std::fs::remove_dir_all(&root);
    }
}
