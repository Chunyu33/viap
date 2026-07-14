// 启动阶段的磁盘扫描策略
// 只负责识别物理介质和决定哪些挂载点需要延后，不参与应用目录递归。

use std::path::{Path, PathBuf};

use sysinfo::{DiskKind, Disks, System};

const COLD_START_UPTIME_SECS: u64 = 60;

#[derive(Debug, Clone, Default)]
pub struct DiskScanPolicy {
    deferred_hdd_mounts: Vec<PathBuf>,
}

impl DiskScanPolicy {
    /// 启动扫描只在冷启动时延后已知机械硬盘，手动刷新永远返回全盘扫描策略。
    pub fn for_scan(is_startup_scan: bool) -> Self {
        if !is_startup_scan || System::uptime() >= COLD_START_UPTIME_SECS {
            return Self::default();
        }

        let mut deferred_hdd_mounts: Vec<PathBuf> = Disks::new_with_refreshed_list()
            .list()
            .iter()
            .filter(|disk| disk.kind() == DiskKind::HDD)
            .map(|disk| disk.mount_point().to_path_buf())
            .filter(|mount| !is_system_mount(mount))
            .collect();

        deferred_hdd_mounts.sort_unstable();
        deferred_hdd_mounts.dedup();

        Self {
            deferred_hdd_mounts,
        }
    }

    /// 判断路径是否位于启动阶段需要延后的机械硬盘挂载点内。
    pub fn should_defer_path(&self, path: &Path) -> bool {
        let normalized_path = normalize_windows_path(path);
        self.deferred_hdd_mounts.iter().any(|mount| {
            let normalized_mount = normalize_windows_path(mount);
            normalized_path == normalized_mount
                || normalized_path.starts_with(&format!("{}\\", normalized_mount))
        })
    }

    /// 返回需要向用户解释的盘符列表，保持稳定排序便于 Toast 阅读。
    pub fn deferred_mount_labels(&self) -> Vec<String> {
        self.deferred_hdd_mounts
            .iter()
            .map(|mount| {
                mount
                    .to_string_lossy()
                    .trim_end_matches(['\\', '/'])
                    .to_string()
            })
            .collect()
    }

    pub fn has_deferred_mounts(&self) -> bool {
        !self.deferred_hdd_mounts.is_empty()
    }
}

fn is_system_mount(mount: &Path) -> bool {
    let normalized = normalize_windows_path(mount);
    normalized == "c:" || normalized.starts_with("c:\\")
}

fn normalize_windows_path(path: &Path) -> String {
    let normalized = path.to_string_lossy().replace('/', "\\").to_lowercase();
    normalized.trim_end_matches('\\').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn path_matching_is_case_insensitive_and_component_aware() {
        // 目录边界必须严格匹配，避免 D:\software2 被误认为 D:\software 的子目录。
        let policy = DiskScanPolicy {
            deferred_hdd_mounts: vec![PathBuf::from("D:\\software")],
        };

        assert!(policy.should_defer_path(Path::new("d:\\software\\app.exe")));
        assert!(policy.should_defer_path(Path::new("D:\\software")));
        assert!(!policy.should_defer_path(Path::new("D:\\software2")));
    }

    #[test]
    fn system_mount_is_not_classified_as_non_system_mount() {
        assert!(is_system_mount(Path::new("C:\\")));
        assert!(!is_system_mount(Path::new("D:\\")));
    }
}
