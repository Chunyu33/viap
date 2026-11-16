//! 迁移目标目录的访问权限修复。
//!
//! 跨盘迁移用 `create_dir_all` 建目录、`CopyFileExW` 复制文件，两者都只继承目标父目录的
//! ACL，不会保留源目录的安全描述符（`CopyFileExW` 明确不复制安全信息）。
//! 非系统盘根目录默认没有 `ALL APPLICATION PACKAGES`（S-1-15-2-1）这条 ACE，
//! 于是迁移后的 Electron/Chromium 应用在沙箱子进程里读不到安装目录中的 DLL，
//! 表现为双击后进程短暂出现即退出（GPU 进程退出码 -2147483645）。
//!
//! 这里按「正常安装目录」的语义补一条只读+执行权限：`C:\Program Files` 自带同样的 ACE。
//! 只增不改，不涉及写权限，也不动所有权。

use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::Command;

use crate::migration::AclRepairReport;
use crate::models::MigrationRecordType;

/// 调用 icacls 时不弹出控制台窗口
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// ALL APPLICATION PACKAGES 的通用 SID。
/// 用 SID 而不是账户名：不同语言的系统上显示名不同，SID 恒定。
const ALL_APPLICATION_PACKAGES_SID: &str = "*S-1-15-2-1";

/// 权限批量修复的幂等标记文件名。
/// 只在全部条目成功后写入，避免每次启动都重复改动 ACL。
const REPAIR_MARKER_FILE: &str = "acl_repair_app_v1.done";

/// 给应用目录补 ALL APPLICATION PACKAGES 的读取+执行权限。
///
/// `(OI)(CI)` 让权限继承到已有和新增的子文件/子目录 —— 实测无需 `/T`，
/// 系统会在写入目录 DACL 时把继承 ACE 传播给既有子项。
/// 重复调用是幂等的：相同 ACE 已存在时 icacls 不会重复添加。
pub(crate) fn ensure_app_container_access(dir: &Path) -> Result<(), String> {
    if !dir.is_dir() {
        return Err(format!("目标目录不存在: {}", dir.display()));
    }

    let dir_text = dir.to_string_lossy().to_string();
    let grant_rule = format!("{}:(OI)(CI)(RX)", ALL_APPLICATION_PACKAGES_SID);
    // /C 让 icacls 在个别子项失败时继续处理，避免一个坏文件导致整目录放弃
    let output = Command::new("icacls")
        .args([dir_text.as_str(), "/grant", grant_rule.as_str(), "/C", "/Q"])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("调用 icacls 失败: {}", e))?;

    if output.status.success() {
        return Ok(());
    }

    // 非 NTFS 目标盘（exFAT / FAT32 移动硬盘）必然失败，属于预期情况，
    // 调用方只记警告，不因此判定迁移失败。
    let stderr = String::from_utf8_lossy(&output.stderr);
    let detail = stderr.trim();
    Err(if detail.is_empty() {
        format!("icacls 退出码 {}", output.status)
    } else {
        format!("icacls 退出码 {}: {}", output.status, detail)
    })
}

/// 启动时批量修复历史迁移记录里的应用目录权限。
///
/// 用于 v1.3.2 及更早版本迁移过的应用：这些目录已经缺 ACE，重新迁移也修不好
/// （源路径已变成链接），只能就地补。
pub(crate) fn repair_migrated_app_permissions() -> AclRepairReport {
    let marker = crate::storage::data_dir::ensure_data_dir().join(REPAIR_MARKER_FILE);
    if marker.exists() {
        return AclRepairReport { skipped: true, ..Default::default() };
    }

    let records = match crate::storage::history::get_migration_history() {
        Ok(records) => records,
        Err(e) => {
            log_warn!("migration", "读取迁移历史失败，跳过权限批量修复: {}", e);
            return AclRepairReport::default();
        }
    };

    let mut report = AclRepairReport::default();
    for record in records.iter() {
        // 只处理仍然生效的应用迁移：用户文件夹不需要沙箱权限，已恢复的记录目标已清空
        if !matches!(&record.record_type, MigrationRecordType::App) || record.status != "active" {
            continue;
        }
        let target = Path::new(&record.target_path);
        if !target.is_dir() {
            continue;
        }
        match ensure_app_container_access(target) {
            Ok(()) => report.repaired += 1,
            Err(e) => {
                report.failed += 1;
                log_warn!(
                    "migration", "应用目录权限修复失败 {}: {}",
                    record.target_path, e
                );
            }
        }
    }

    // 有失败就不写标记：下次启动会重试，避免一次权限不足导致永久漏修
    if report.failed == 0 {
        if let Err(e) = std::fs::write(&marker, b"v1") {
            log_warn!("migration", "写入权限修复标记失败: {}", e);
        }
    }

    report
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 授权应写入继承 ACE，且重复调用不报错（幂等）。
    #[test]
    fn grant_is_idempotent_and_inherits() {
        let dir = std::env::temp_dir().join("viap_acl_grant_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).expect("创建测试目录失败");
        std::fs::write(dir.join("sub").join("a.dll"), b"x").expect("写入测试文件失败");

        ensure_app_container_access(&dir).expect("首次授权失败");
        ensure_app_container_access(&dir).expect("重复授权应成功");

        let output = Command::new("icacls").arg(&dir).output().expect("查询 ACL 失败");
        let text = String::from_utf8_lossy(&output.stdout);
        // 不同语言系统下账户显示名不同，SID 恒定；两者任一命中即可
        assert!(
            text.contains("S-1-15-2-1") || text.contains("ALL APPLICATION PACKAGES"),
            "未找到 ALL APPLICATION PACKAGES: {}",
            text
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 目录不存在时应返回可读错误，而不是静默成功。
    #[test]
    fn missing_directory_reports_error() {
        let missing = std::env::temp_dir().join("viap_acl_missing_dir");
        let _ = std::fs::remove_dir_all(&missing);
        assert!(ensure_app_container_access(&missing).is_err());
    }
}
