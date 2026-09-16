// Viap — 专业的 Windows 存储重定向工具
//
// 模块结构：
// - models:           共享数据结构（跨模块基础类型）
// - utils:            文件系统工具函数
// - system/           系统接口（磁盘信息、图标提取）
// - storage/          存储层（数据目录配置、迁移历史持久化）
// - folder_manager/   文件夹管理（发现、迁移、恢复大文件夹）
// - app_manager/      应用管理（扫描、迁移、卸载、检测）

#[macro_use]
mod log_macros;

mod app_manager;
mod migration;
mod models;
mod utils;
mod system;
mod storage;
mod folder_manager;
mod integrity;

use std::sync::atomic::Ordering;

use crate::models::*;
use crate::app_manager::uninstaller;
use tauri::Manager;

fn empty_icon_response() -> tauri::http::Response<Vec<u8>> {
    tauri::http::Response::builder()
        .status(204)
        .header("content-type", "image/png")
        .body(Vec::new())
        .unwrap()
}

fn icon_response(png_bytes: Vec<u8>) -> tauri::http::Response<Vec<u8>> {
    if png_bytes.is_empty() {
        return empty_icon_response();
    }
    tauri::http::Response::builder()
        .status(200)
        .header("content-type", "image/png")
        .header("cache-control", "public, max-age=604800")
        .body(png_bytes)
        .unwrap()
}

// ============================================================================
// Tauri 命令 — 系统信息
// ============================================================================

/// 获取 Viap 自身的安装目录，前端用于禁用自身的迁移/卸载按钮
#[tauri::command]
fn get_viap_install_path() -> Result<String, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let parent = exe.parent().ok_or("无法获取 Viap 安装目录".to_string())?;
    Ok(parent.to_string_lossy().to_string())
}

/// 前端首帧挂载后再显示窗口，避免 WebView 初始化期间暴露白屏。
#[tauri::command]
fn frontend_ready(app_handle: tauri::AppHandle) -> Result<(), String> {
    if let Some(window) = app_handle.get_webview_window("main") {
        window.show().map_err(|e| format!("显示主窗口失败: {}", e))?;
        window.set_focus().map_err(|e| format!("聚焦主窗口失败: {}", e))?;
    }
    Ok(())
}

/// 返回当前构建是否为便携版，前端据此关闭自动更新并展示手动下载入口。
#[tauri::command]
fn is_portable_build() -> bool {
    cfg!(feature = "portable")
}

/// 校验当前运行 exe，返回结构化状态让前端区分网络、签名和篡改结果。
#[tauri::command]
async fn verify_file_integrity(app_handle: tauri::AppHandle) -> integrity::IntegrityCheckResult {
    integrity::verify_file_integrity(app_handle).await
}

// ============================================================================
// Tauri 命令 — 应用管理（委托给 app_manager 子模块）
// ============================================================================

#[tauri::command]
fn get_installed_apps() -> Result<Vec<InstalledApp>, String> {
    app_manager::scanner::get_installed_apps()
}

/// 流式扫描应用列表
/// 扫描结果通过 `scan-progress` 事件逐阶段推送，不等待全部完成
/// 前端通过 listen('scan-progress') 接收增量数据
///
/// 返回值：扫描完成后的完整应用列表（用于兜底校验）
#[tauri::command]
async fn get_installed_apps_stream(
    app_handle: tauri::AppHandle,
    use_snapshot: Option<bool>,
    force_refresh: Option<bool>,
) -> Result<Vec<InstalledApp>, String> {
    if force_refresh.unwrap_or(false) {
        app_manager::cache::invalidate();
    }

    // 快速路径：内存缓存命中则直接 emit done 事件（含完整列表），不重新扫描
    if let Some(cached) = app_manager::cache::get_cached() {
        let total = cached.len();
        use tauri::Emitter;
        let _ = app_handle.emit("scan-progress", app_manager::scanner::ScanProgressEvent {
            phase: "done".to_string(),
            apps: cached.clone(),
            icon_updates: vec![],
            size_updates: vec![],
            total_count: total,
            is_final: true,
        });
        return Ok(cached);
    }

    // 缓存未命中：在阻塞线程池中执行流式扫描
    let app_handle_clone = app_handle.clone();
    let use_snapshot = use_snapshot.unwrap_or(true);
    let result = tauri::async_runtime::spawn_blocking(move || {
        app_manager::scanner::SCANNER.scan_all_streaming(&app_handle_clone, use_snapshot)
    })
    .await
    .map_err(|e| format!("扫描线程异常: {}", e))??;

    // 写入全局缓存，后续调用 get_or_scan / get_cached 可命中
    app_manager::cache::set_cache(result.clone());

    Ok(result)
}

/// 强制刷新应用列表：清空内存缓存并触发全量扫描
#[tauri::command]
fn refresh_apps() -> Result<Vec<InstalledApp>, String> {
    app_manager::cache::refresh()
}

#[tauri::command]
fn get_app_size(install_location: String) -> Result<u64, String> {
    app_manager::scanner::get_app_size(install_location)
}

#[tauri::command]
fn check_process_locks(source_path: String) -> Result<ProcessLockResult, String> {
    app_manager::scanner::check_process_locks(source_path)
}

/// 将阻塞型 IO（目录遍历、文件复制、联接创建）移至专用线程池
/// 主 IPC 线程保持空闲以处理进度事件和取消请求，消除 UI 假死
#[tauri::command]
async fn migrate_app(
    app_name: String,
    source: String,
    target_parent: String,
    force_overwrite: Option<bool>,
    user_confirmed_warning: Option<bool>,
    state: tauri::State<'_, MigrationState>,
    app_handle: tauri::AppHandle,
) -> Result<MigrationResult, String> {
    state.cancel_flag.store(false, Ordering::SeqCst);
    let source_clone = source.clone();
    let cancel_flag = state.cancel_flag.clone();
    let force = force_overwrite.unwrap_or(false);
    let confirmed = user_confirmed_warning.unwrap_or(false);

    let result = tauri::async_runtime::spawn_blocking(move || {
        migration::migrate_app(
            app_name, source, target_parent, &cancel_flag, &app_handle,
            MigrationRecordType::App, force, confirmed,
        )
    }).await.map_err(|e| format!("迁移线程异常: {}", e))?;

    let result = result?;

    if result.success {
        if let Some(ref new_path) = result.new_path {
            app_manager::cache::on_app_migrated(&source_clone, new_path);
        }
    }
    Ok(result)
}

#[tauri::command]
async fn migrate_special_folder(
    app_name: String,
    source_path: String,
    target_dir: String,
    state: tauri::State<'_, MigrationState>,
    app_handle: tauri::AppHandle,
) -> Result<MigrationResult, String> {
    state.cancel_flag.store(false, Ordering::SeqCst);
    let cancel_flag = state.cancel_flag.clone();

    // force_overwrite=false：文件夹迁移不自动覆盖残留目录，保持保护逻辑
    tauri::async_runtime::spawn_blocking(move || {
        app_manager::detector::migrate_special_folder(
            app_name, source_path, target_dir, &cancel_flag, &app_handle, false, false,
        )
    }).await.map_err(|e| format!("迁移线程异常: {}", e))?
}

#[tauri::command]
fn cancel_migration(state: tauri::State<'_, MigrationState>) -> Result<(), String> {
    state.cancel_flag.store(true, Ordering::SeqCst);
    Ok(())
}

// ============================================================================
// Tauri 命令 — 卸载（委托给 app_manager::uninstaller）
// ============================================================================

#[tauri::command]
fn preview_uninstall(input: uninstaller::UninstallInput) -> Result<uninstaller::UninstallPreview, String> {
    uninstaller::preview_uninstall(input)
}

#[tauri::command]
fn preview_force_remove(input: uninstaller::UninstallInput) -> Result<Vec<uninstaller::LeftoverItem>, String> {
    uninstaller::preview_force_remove(input)
}

#[tauri::command]
fn force_remove_application(input: uninstaller::UninstallInput) -> Result<uninstaller::UninstallResult, String> {
    let install_location = input.install_location.clone();
    let result = uninstaller::force_remove_application(input)?;
    // 强删成功后从缓存中移除，避免显示已不存在的应用
    if result.success && result.application_removed {
        if let Some(ref loc) = install_location {
            app_manager::cache::on_app_uninstalled(loc);
        }
    }
    Ok(result)
}

#[tauri::command]
async fn uninstall_application(input: uninstaller::UninstallInput) -> Result<uninstaller::UninstallResult, String> {
    let install_location = input.install_location.clone();
    let result = uninstaller::uninstall_application(input).await?;
    // 正常卸载成功后同样从缓存移除
    if result.success {
        if let Some(ref loc) = install_location {
            app_manager::cache::on_app_uninstalled(loc);
        }
    }
    Ok(result)
}

#[tauri::command]
async fn scan_app_residue(
    app_name: String, publisher: Option<String>, install_location: Option<String>,
) -> Result<Vec<uninstaller::LeftoverItem>, String> {
    // 残留扫描包含大量文件系统和注册表读取，放入阻塞线程池避免拖住 Tauri 主线程。
    tauri::async_runtime::spawn_blocking(move || {
        uninstaller::scan_app_residue(app_name, publisher, install_location)
    })
    .await
    .map_err(|error| format!("残留扫描线程异常: {}", error))?
}

#[tauri::command]
fn execute_cleanup(
    items: Vec<String>, app_name: Option<String>, publisher: Option<String>,
) -> Result<uninstaller::CleanupResult, String> {
    uninstaller::execute_cleanup(items, app_name, publisher)
}

// ============================================================================
// Tauri 应用入口
// ============================================================================

/// 便携版把 WebView2 的用户数据目录也放到程序目录下
///
/// 数据目录与指针文件已经避开系统盘，但 WebView2 默认把浏览器缓存、localStorage
/// 放在 %LOCALAPPDATA%\<标识>\EBWebView。不重定向的话便携版仍会在 C 盘留下目录。
/// 必须在创建 WebView2 环境之前设置环境变量，因此放在 run() 的最前面。
#[cfg(feature = "portable")]
fn redirect_portable_webview_data_dir() {
    if std::env::var_os("WEBVIEW2_USER_DATA_FOLDER").is_some() {
        return;
    }

    let program_dir = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(std::path::Path::to_path_buf));
    let Some(program_dir) = program_dir else {
        log_warn!("webview", "无法获取程序目录，WebView2 数据仍写入系统盘");
        return;
    };

    let webview_dir = program_dir.join("webview");
    match std::fs::create_dir_all(&webview_dir) {
        Ok(()) => std::env::set_var("WEBVIEW2_USER_DATA_FOLDER", &webview_dir),
        // 程序目录不可写（如放在只读位置）时保持系统默认，只记录原因
        Err(error) => log_warn!(
            "webview",
            "无法创建 WebView2 数据目录 {}: {}",
            webview_dir.display(),
            error
        ),
    }
}

/// 在窗口显示前套用记忆的窗口尺寸
///
/// 窗口创建时用的是配置里的默认尺寸，这里在隐藏状态下调整，避免启动瞬间
/// 先出现默认尺寸再跳到用户尺寸。换过显示器或降低了分辨率时会按当前显示器收敛，
/// 防止记忆中的大窗口超出屏幕。
fn apply_saved_window_size(window: &tauri::WebviewWindow) {
    let Some((saved_width, saved_height)) = storage::user_settings::saved_window_size() else {
        return;
    };

    let mut logical_width = saved_width as f64;
    let mut logical_height = saved_height as f64;

    match window.current_monitor() {
        Ok(Some(monitor)) => {
            let scale = monitor.scale_factor();
            if scale > 0.0 {
                // 预留一点高度给任务栏，避免窗口底部被遮住
                const TASKBAR_ALLOWANCE: f64 = 48.0;
                logical_width = logical_width.min(monitor.size().width as f64 / scale);
                logical_height =
                    logical_height.min(monitor.size().height as f64 / scale - TASKBAR_ALLOWANCE);
            }
        }
        Ok(None) => {}
        Err(error) => log_warn!("window", "读取当前显示器信息失败: {}", error),
    }

    // 与 tauri.conf.json 的 minWidth/minHeight 保持一致
    logical_width = logical_width.max(800.0);
    logical_height = logical_height.max(540.0);

    if let Err(error) = window.set_size(tauri::LogicalSize::new(logical_width, logical_height)) {
        log_warn!("window", "恢复记忆窗口尺寸失败: {}", error);
        return;
    }
    if let Err(error) = window.center() {
        log_warn!("window", "窗口居中失败: {}", error);
    }
}

/// 上次启动时迁移中断的恢复提示（只提示一次，取走即清空）
static PENDING_MIGRATION_NOTICE: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// 取走启动时的迁移中断提示，供前端弹一次 toast
#[tauri::command]
fn take_pending_migration_notice() -> Option<String> {
    PENDING_MIGRATION_NOTICE
        .lock()
        .ok()
        .and_then(|mut notice| notice.take())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    #[cfg(feature = "portable")]
    redirect_portable_webview_data_dir();

    let builder = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init());

    // 便携版不注册 updater 插件，避免误触发安装器更新或写入安装目录外的更新状态。
    #[cfg(not(feature = "portable"))]
    let builder = builder.plugin(tauri_plugin_updater::Builder::new().build());

    builder
        .plugin(tauri_plugin_process::init())
        .setup(|app| {
            // 窗口尚未显示，此时套用记忆尺寸不会出现尺寸跳动
            if let Some(window) = app.get_webview_window("main") {
                apply_saved_window_size(&window);
            }

            // 上次迁移在"改名 → 建链接"之间被中断时，先把备份还原回原路径，
            // 否则原路径缺失会让应用直接不可用；提示交给前端弹一次 toast
            if let Some(notice) = storage::pending_migration::recover_interrupted_migration() {
                log_warn!("migration", "{}", notice);
                if let Ok(mut slot) = PENDING_MIGRATION_NOTICE.lock() {
                    *slot = Some(notice);
                }
            }

            let app_handle = app.handle().clone();
            std::thread::spawn(move || {
                std::thread::sleep(std::time::Duration::from_secs(5));
                // 前端 ready 信号异常时兜底显示窗口，避免用户误以为程序打不开。
                if let Some(window) = app_handle.get_webview_window("main") {
                    if !window.is_visible().unwrap_or(false) {
                        let _ = window.show();
                    }
                }
            });
            Ok(())
        })
        .register_asynchronous_uri_scheme_protocol("viap-icon", |_ctx, request, responder| {
            let encoded_path = request.uri().path().to_string();
            std::thread::spawn(move || {
                // 图标请求来自 WebView 的 img 标签，失败时返回空响应即可保留占位图。
                let response = crate::app_manager::snapshot::decode_icon_path(&encoded_path)
                    .map(|icon_path| crate::system::icon::extract_icon_png_bytes(&icon_path))
                    .map(icon_response)
                    .unwrap_or_else(empty_icon_response);
                responder.respond(response);
            });
        })
        .manage(MigrationState::default())
        .manage(LinkRecoveryState::default())
        .invoke_handler(tauri::generate_handler![
            // 系统接口
            system::disk_usage::get_disk_usage,
            frontend_ready,
            take_pending_migration_notice,
            is_portable_build,
            get_viap_install_path,
            verify_file_integrity,
            // 存储层 — 数据目录
            storage::data_dir::initialize_storage,
            storage::data_dir::get_data_dir_info,
            storage::data_dir::get_config_files,
            storage::data_dir::set_data_dir,
            storage::user_settings::get_user_settings,
            storage::user_settings::save_user_settings,
            storage::user_settings::save_window_state,
            // 文件夹管理
            folder_manager::get_large_folders,
            folder_manager::start_folder_size_scan,
            folder_manager::start_app_data_size_scan,
            folder_manager::migrate_large_folder,
            folder_manager::add_custom_folder,
            folder_manager::remove_custom_folder,
            folder_manager::restore_large_folder,
            folder_manager::get_app_data_templates,
            folder_manager::save_app_data_templates,
            // 存储层 — 历史记录
            storage::history::get_migration_history,
            storage::history::get_migrated_paths,
            storage::history::restore_app,
            storage::history::cleanup_broken_record,
            storage::history::delete_migration_record,
            storage::history::remigrate_ghost_link,
            storage::history::check_link_status,
            storage::history::clean_ghost_links,
            storage::history::preview_ghost_links,
            storage::history::get_migration_stats,
            storage::history::start_recovered_size_scan,
            storage::history::export_history,
            storage::history::import_history,
            storage::history::open_data_dir,
            storage::history::open_folder,
            // 存储层 — 迁移记录重建（原路径链接识别）与镜像备份
            storage::link_recovery::scan_migration_links,
            storage::link_recovery::import_recovered_links,
            storage::link_recovery::cancel_link_recovery,
            storage::mirror::get_mirror_backup_info,
            storage::mirror::import_mirror_backup,
            storage::mirror::backup_now,
            storage::mirror::open_mirror_dir,
            // 存储层 — 操作日志
            storage::operation_log::get_operation_logs,
            // 应用管理
            get_installed_apps,
            get_installed_apps_stream,
            refresh_apps,
            get_app_size,
            check_process_locks,
            migrate_app,
            migrate_special_folder,
            cancel_migration,
            // 卸载
            preview_uninstall,
            preview_force_remove,
            force_remove_application,
            uninstall_application,
            scan_app_residue,
            execute_cleanup,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
