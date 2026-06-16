// 应用管理模块
// 负责应用扫描、迁移和卸载能力

#[macro_use]
mod log_macros;
pub mod cache;
pub mod scanner;
pub mod snapshot;
pub mod migration;
pub mod uninstaller;
pub mod detector;
