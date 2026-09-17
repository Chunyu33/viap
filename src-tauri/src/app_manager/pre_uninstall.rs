// 卸载前信息聚合
//
// 把"用户点卸载/清理前需要知道的信息"合并成一次调用，避免多轮 IPC 往返：
// - 是否为 MS Store / UWP 包（存在时走系统组件移除）
// - 是否残留服务 / 驱动 / 计划任务（只提示，不自动删除）
//
// 这些检测都涉及注册表枚举或 PowerShell，属于阻塞操作，因此放在阻塞线程池执行。

use serde::Serialize;

use super::{appx, traces};

/// 卸载前提示信息
#[derive(Debug, Serialize)]
pub struct PreUninstallInfo {
    /// 匹配到的 Appx 包（MS Store 应用）；多个匹配时为空，交由用户自行确认
    pub store_package: Option<appx::AppxPackage>,
    /// 服务 / 驱动 / 计划任务等系统痕迹
    pub system_traces: Vec<traces::SystemTrace>,
}

/// 获取卸载前提示信息
#[tauri::command]
pub async fn get_pre_uninstall_info(
    app_name: String,
    install_location: Option<String>,
) -> Result<PreUninstallInfo, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let location = install_location
            .as_deref()
            .map(std::path::Path::new)
            .filter(|path| !path.as_os_str().is_empty());

        PreUninstallInfo {
            store_package: appx::resolve_package(&app_name, location),
            system_traces: traces::detect_system_traces(&app_name, install_location.as_deref()),
        }
    })
    .await
    .map_err(|error| format!("获取卸载前信息失败: {}", error))
}
