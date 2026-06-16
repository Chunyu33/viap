// 内存级应用缓存
// 避免 Tab 切换或重复操作时触发全量扫描，迁移/卸载后增量更新

use crate::app_manager::scanner::SCANNER;
use crate::models::InstalledApp;
use std::sync::RwLock;
use std::time::Instant;

// ============================================================================
// AppCache — 全量应用快照缓存
// ============================================================================

pub struct AppCache {
    /// 全量应用列表（含图标 Base64）
    apps: Vec<InstalledApp>,
    /// 上次全量扫描时间
    last_scan_time: Instant,
    /// 脏标记：true 表示缓存失效，下次访问需重新扫描
    is_dirty: bool,
}

impl AppCache {
    fn new() -> Self {
        Self {
            apps: Vec::new(),
            last_scan_time: Instant::now(),
            is_dirty: true, // 初始状态脏，首次访问触发扫描
        }
    }

    fn is_valid(&self) -> bool {
        !self.is_dirty
    }

    fn invalidate(&mut self) {
        self.is_dirty = true;
    }
}

// ============================================================================
// 全局单例
// ============================================================================

lazy_static::lazy_static! {
    static ref APP_CACHE: RwLock<AppCache> = RwLock::new(AppCache::new());
}

// ============================================================================
// 公共 API
// ============================================================================

/// 尝试获取内存缓存，未命中返回 None，不触发扫描
/// 用于流式扫描命令快速返回，避免重复全量扫描
pub fn get_cached() -> Option<Vec<InstalledApp>> {
    let cache = APP_CACHE.read().unwrap();
    if cache.is_valid() {
        Some(cache.apps.clone())
    } else {
        None
    }
}

/// 将扫描结果写入缓存，供后续调用 get_or_scan / get_cached 命中
pub fn set_cache(apps: Vec<InstalledApp>) {
    let mut cache = APP_CACHE.write().unwrap();
    let mut apps = apps;
    crate::app_manager::snapshot::attach_icon_urls(&mut apps);
    crate::app_manager::snapshot::save_snapshot(&apps);
    cache.apps = apps;
    cache.last_scan_time = Instant::now();
    cache.is_dirty = false;
}

/// 仅标记缓存失效，不立即扫描；用于前端流式刷新重新接收阶段事件。
pub fn invalidate() {
    let mut cache = APP_CACHE.write().unwrap();
    cache.invalidate();
}

/// 获取应用列表：缓存有效时直接返回内存数据，否则触发全量扫描
pub fn get_or_scan() -> Result<Vec<InstalledApp>, String> {
    // 快速路径：缓存命中，仅持有读锁
    {
        let cache = APP_CACHE.read().unwrap();
        if cache.is_valid() {
            return Ok(cache.apps.clone());
        }
    }

    let mut apps = SCANNER.scan_all()?;

    // 兜底：从迁移元数据补全扫描器遗漏的已迁移应用（如绿色软件）
    // 仅补充扫描结果中不存在的路径，避免重复
    let existing: std::collections::HashSet<String> = apps
        .iter()
        .map(|a| a.install_location.to_lowercase())
        .collect();
    let failsafe = crate::storage::migrated_app_metadata::generate_failsafe_apps(&existing);
    apps.extend(failsafe);
    crate::app_manager::snapshot::attach_icon_urls(&mut apps);

    // 图标复用：路径未变的条目保留原有 Base64，减少 CPU 开销
    // 仅在新图标为空且旧缓存非空时才回填，避免用 scan_all_streaming 返回的空值
    // 覆盖 scan_all() 中 extract_icons_parallel 刚提取的有效图标
    {
        let icon_cache: std::collections::HashMap<
            (String, String),
            (String, String),
        > = {
            let cache = APP_CACHE.read().unwrap();
            cache.apps.iter().map(|a| {
                ((a.install_location.clone(), a.display_icon.clone()),
                 (a.icon_base64.clone(), a.icon_url.clone()))
            }).collect()
        }; // 读锁在此释放

        for app in &mut apps {
            if app.icon_base64.is_empty() {
                if let Some((b64, url)) = icon_cache.get(
                    &(app.install_location.clone(), app.display_icon.clone())
                ) {
                    if !b64.is_empty() {
                        app.icon_base64 = b64.clone();
                    }
                    if !url.is_empty() {
                        app.icon_url = url.clone();
                    }
                }
            }
        }
    }

    // 写入缓存
    {
        let mut cache = APP_CACHE.write().unwrap();
        cache.apps = apps.clone();
        cache.last_scan_time = Instant::now();
        cache.is_dirty = false;
    }
    crate::app_manager::snapshot::save_snapshot(&apps);

    Ok(apps)
}

/// 强制刷新：清空缓存并触发全量扫描
pub fn refresh() -> Result<Vec<InstalledApp>, String> {
    {
        let mut cache = APP_CACHE.write().unwrap();
        cache.invalidate();
    }
    get_or_scan()
}

/// 迁移成功后标记缓存为脏，触发下轮全量扫描以同步最新注册表路径
/// 不修改 install_location —— 目录联接使 OS 仍以原路径访问，迁移记录 key 也基于原路径
pub fn on_app_migrated(_old_path: &str, _new_path: &str) {
    let mut cache = APP_CACHE.write().unwrap();
    cache.invalidate();
}

/// 卸载成功后从缓存中移除
pub fn on_app_uninstalled(install_location: &str) {
    let mut cache = APP_CACHE.write().unwrap();
    cache
        .apps
        .retain(|a| a.install_location != install_location);
}
