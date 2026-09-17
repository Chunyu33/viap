// 应用管理模块
// 负责应用扫描、迁移和卸载能力

pub mod appx;
pub mod cache;
pub mod disk_scan_policy;
pub mod pre_uninstall;
pub mod scanner;
pub mod snapshot;
pub mod traces;
pub mod uninstall_snapshot;
pub mod uninstaller;
pub mod detector;
