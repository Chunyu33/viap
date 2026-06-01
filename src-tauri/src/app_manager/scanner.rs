// 应用扫描模块 — AppScanner 架构
//
// 三级检索模型（Tier 1 → 2 → 3）：
//   Tier 1: 深度注册表解析（命中率 ~85%，<200ms）
//   Tier 2: LNK 快捷方式解析（命中率 ~10%，<300ms）
//   Tier 3: 受限文件系统扫描（命中率 ~5%，<500ms）
//
// 噪声消减：PE 元数据校验 + Shannon 熵值检测 + 硬黑名单 + 系统组件过滤
// 性能：rayon 并行化 + 延迟大小计算 + MTime 增量缓存 + 提前终止

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Instant;

use serde::Serialize;
use tauri::Emitter;

use crate::models::{InstalledApp, ProcessLockResult};
use rayon::prelude::*;
use sysinfo::System;

// ============================================================================
// 常量
// ============================================================================

/// 注册表扫描结果缓存 TTL（秒）
const REGISTRY_CACHE_TTL_SECS: u64 = 30;
/// 熵值阈值：Shannon 熵 >= 此值视为随机文件名
const ENTROPY_THRESHOLD: f64 = 3.5;
/// 注册表应用数达到此阈值后跳过 Tier 3 文件系统扫描（Tier 2 LNK 始终执行）
const EARLY_EXIT_APP_COUNT: usize = 1000;
/// 评分阈值
const SCORE_THRESHOLD: f32 = 0.35;

lazy_static::lazy_static! {
    /// 缓存的下载目录路径
    static ref DOWNLOADS_DIR_LOWER: Option<String> =
        dirs::download_dir().map(|p| p.to_string_lossy().to_lowercase());
}

#[cfg(windows)]
use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
#[cfg(windows)]
use winreg::RegKey;
#[cfg(windows)]
use winreg::HKEY;

// ============================================================================
// 流式扫描事件类型
// ============================================================================

/// 流式扫描进度事件 payload
/// 事件名：`scan-progress`
#[derive(Debug, Clone, Serialize)]
pub struct ScanProgressEvent {
    /// 当前阶段："tier1" | "tier2" | "tier3" | "icons" | "done"
    pub phase: String,
    /// 本批新增应用列表（增量，不含历史已推送的）
    pub apps: Vec<InstalledApp>,
    /// 图标批次更新（key=install_location, value=icon_base64）
    /// 仅 phase="icons" 时有值，其余阶段为空
    pub icon_updates: Vec<IconUpdate>,
    /// 当前累计应用总数（含本批）
    pub total_count: usize,
    /// 是否为最终完成事件
    pub is_final: bool,
}

/// 单个图标更新
#[derive(Debug, Clone, Serialize)]
pub struct IconUpdate {
    pub install_location: String,
    pub icon_base64: String,
}

// ============================================================================
// AppScanner 结构体
// ============================================================================

/// 应用扫描器——封装三级检索模型、缓存与增量扫描能力
pub struct AppScanner {
    /// 上次全量扫描完成时间
    last_full_scan: std::sync::Mutex<Option<Instant>>,
    /// 注册表扫描结果缓存（30s TTL）
    registry_cache: std::sync::Mutex<Option<(Instant, Vec<InstalledApp>)>>,
}

impl AppScanner {
    pub fn new() -> Self {
        Self {
            last_full_scan: std::sync::Mutex::new(None),
            registry_cache: std::sync::Mutex::new(None),
        }
    }

    /// 全量扫描：Tier 1 → Tier 2 → Tier 3
    pub fn scan_all(&self) -> Result<Vec<InstalledApp>, String> {
        let total_start = Instant::now();

        // Tier 1：深度注册表解析
        let t1_start = Instant::now();
        let mut apps = self.scan_registry_deep()?;
        let t1_ms = t1_start.elapsed().as_millis();
        orbit_log!("INFO", "scanner", "Tier1 注册表扫描完成: {} 个应用, {}ms", apps.len(), t1_ms);

        // Tier 2：LNK 快捷方式解析（始终执行——准确度极高、耗时极短）
        let existing_paths: HashSet<String> = apps
            .iter()
            .map(|a| normalize_path(&a.install_location))
            .collect();

        let t2_start = Instant::now();
        let lnk_apps = self.scan_lnk_shortcuts(&existing_paths);
        let t2_ms = t2_start.elapsed().as_millis();
        orbit_log!("INFO", "scanner", "Tier2 LNK扫描完成: {} 个应用, {}ms", lnk_apps.len(), t2_ms);

        let mut existing_paths: HashSet<String> = existing_paths
            .into_iter()
            .chain(lnk_apps.iter().map(|a| normalize_path(&a.install_location)))
            .collect();
        // 迁移后安装路径变为目录联接，将联接目标物理路径也加入已知集合
        // 避免 Tier3 在目标盘重新发现同一应用导致重复条目
        let symlink_targets: Vec<String> = existing_paths
            .iter()
            .filter_map(|p| {
                let path = Path::new(p);
                if path.is_symlink() {
                    std::fs::read_link(path).ok()
                        .map(|t| normalize_path(&t.to_string_lossy()))
                } else {
                    None
                }
            })
            .collect();
        for target in symlink_targets {
            existing_paths.insert(target);
        }
        apps.extend(lnk_apps);

        // 提前终止：注册表覆盖足够多应用时仅跳过 Tier 3 文件系统扫描
        if apps.len() >= EARLY_EXIT_APP_COUNT {
            orbit_log!("INFO", "scanner", "应用数 >= {}，跳过 Tier3 文件系统扫描", EARLY_EXIT_APP_COUNT);
            apps.sort_by(|a, b| a.display_name.to_lowercase().cmp(&b.display_name.to_lowercase()));
            self.extract_icons_parallel(&mut apps);
            // 写入缓存
            if let Ok(mut cache) = self.registry_cache.lock() {
                *cache = Some((Instant::now(), apps.clone()));
            }
            *self.last_full_scan.lock().unwrap() = Some(Instant::now());
            orbit_log!("INFO", "scanner", "全量扫描完成(提前终止): {}ms", total_start.elapsed().as_millis());
            return Ok(apps);
        }

        // Tier 3：受限文件系统扫描
        let t3_start = Instant::now();
        let fs_apps = self.scan_filesystem_constrained(&existing_paths);
        let t3_ms = t3_start.elapsed().as_millis();
        orbit_log!("INFO", "scanner", "Tier3 文件系统扫描完成: {} 个应用, {}ms", fs_apps.len(), t3_ms);
        apps.extend(fs_apps);

        // 后处理：去重、排序、图标、缓存
        dedup_subdirectory_apps(&mut apps);
        apps.sort_by(|a, b| a.display_name.to_lowercase().cmp(&b.display_name.to_lowercase()));
        self.extract_icons_parallel(&mut apps);

        // 写入缓存
        if let Ok(mut cache) = self.registry_cache.lock() {
            *cache = Some((Instant::now(), apps.clone()));
        }
        *self.last_full_scan.lock().unwrap() = Some(Instant::now());

        orbit_log!("INFO", "scanner", "全量扫描完成: {} 个应用, 总耗时 {}ms", apps.len(), total_start.elapsed().as_millis());
        Ok(apps)
    }

    /// 流式全量扫描：每个 Tier 完成后立即通过 Tauri 事件推送，不等待全部完成
    ///
    /// 事件名：`scan-progress`，payload：`ScanProgressEvent`
    ///
    /// 流程：
    /// 1. Tier 1 注册表扫描完成 → emit phase="tier1"，apps=注册表应用列表
    /// 2. Tier 2 LNK 扫描完成  → emit phase="tier2"，apps=LNK 新增应用（去重后）
    /// 3. Tier 3 文件系统扫描完成（若未提前终止）→ emit phase="tier3"，apps=FS 新增应用
    /// 4. 图标提取（全部应用，分批，每批 20 个）→ 每批 emit phase="icons"
    /// 5. 完成 → emit phase="done"，is_final=true
    ///
    /// 同时将最终结果写入内存缓存（通过 cache 模块的全局 APP_CACHE）
    pub fn scan_all_streaming(&self, app_handle: &tauri::AppHandle) -> Result<Vec<InstalledApp>, String> {
        let total_start = Instant::now();
        let mut all_apps: Vec<InstalledApp> = Vec::new();

        // ── Tier 1：注册表 ──────────────────────────────────────────────
        let t1_apps = self.scan_registry_deep()?;
        orbit_log!("INFO", "scanner", "Tier1 完成: {} 个, {}ms", t1_apps.len(), total_start.elapsed().as_millis());

        let _ = app_handle.emit("scan-progress", ScanProgressEvent {
            phase: "tier1".to_string(),
            apps: t1_apps.clone(),
            icon_updates: vec![],
            total_count: t1_apps.len(),
            is_final: false,
        });

        all_apps.extend(t1_apps);

        // ── Tier 2：LNK 快捷方式 ────────────────────────────────────────
        let mut existing_paths: HashSet<String> = all_apps
            .iter()
            .map(|a| normalize_path(&a.install_location))
            .collect();

        // 解析 symlink 目标，防止 Tier3 在目标盘重复发现
        let symlink_targets: Vec<String> = existing_paths
            .iter()
            .filter_map(|p| {
                let path = std::path::Path::new(p);
                if path.is_symlink() {
                    std::fs::read_link(path).ok()
                        .map(|t| normalize_path(&t.to_string_lossy()))
                } else { None }
            })
            .collect();
        for t in symlink_targets { existing_paths.insert(t); }

        let t2_apps = self.scan_lnk_shortcuts(&existing_paths);
        orbit_log!("INFO", "scanner", "Tier2 完成: {} 个新应用", t2_apps.len());

        let _ = app_handle.emit("scan-progress", ScanProgressEvent {
            phase: "tier2".to_string(),
            apps: t2_apps.clone(),
            icon_updates: vec![],
            total_count: all_apps.len() + t2_apps.len(),
            is_final: false,
        });

        for app in &t2_apps {
            existing_paths.insert(normalize_path(&app.install_location));
        }
        all_apps.extend(t2_apps);

        // ── Tier 3：文件系统扫描（未达提前终止阈值时执行）──────────────
        if all_apps.len() < EARLY_EXIT_APP_COUNT {
            let t3_apps = self.scan_filesystem_constrained(&existing_paths);
            orbit_log!("INFO", "scanner", "Tier3 完成: {} 个新应用", t3_apps.len());

            let _ = app_handle.emit("scan-progress", ScanProgressEvent {
                phase: "tier3".to_string(),
                apps: t3_apps.clone(),
                icon_updates: vec![],
                total_count: all_apps.len() + t3_apps.len(),
                is_final: false,
            });

            all_apps.extend(t3_apps);
        } else {
            orbit_log!("INFO", "scanner", "应用数 >= {}，跳过 Tier3", EARLY_EXIT_APP_COUNT);
            let _ = app_handle.emit("scan-progress", ScanProgressEvent {
                phase: "tier3".to_string(),
                apps: vec![],
                icon_updates: vec![],
                total_count: all_apps.len(),
                is_final: false,
            });
        }

        // 后处理（去重、排序）——在图标提取前完成
        dedup_subdirectory_apps(&mut all_apps);
        all_apps.sort_by(|a, b| a.display_name.to_lowercase().cmp(&b.display_name.to_lowercase()));

        // 兜底补全：扫描器遗漏的已迁移绿色/便携软件（无注册表条目）
        // get_or_scan() 有此步骤，scan_all_streaming() 也必须对称执行，
        // 否则这些应用在初始加载时缺失，仅在手动刷新后才出现
        {
            let existing: HashSet<String> = all_apps
                .iter()
                .map(|a| a.install_location.to_lowercase())
                .collect();
            let failsafe = crate::storage::migrated_app_metadata::generate_failsafe_apps(&existing);
            all_apps.extend(failsafe);
        }
        // 兜底应用可能导致排序变化，重新排序
        all_apps.sort_by(|a, b| a.display_name.to_lowercase().cmp(&b.display_name.to_lowercase()));

        // ── 图标提取（分批，每批 20 个，边提取边推送）──────────────────
        // 图标提取是 IO 密集操作，不用 rayon 并行（机械盘并行反而更慢）
        // 改为顺序分批，每批完成后立即推送，让前端能逐渐渲染图标
        let icon_batch_size = 20;
        let total_app_count = all_apps.len();
        let total_batches = (total_app_count + icon_batch_size - 1) / icon_batch_size;
        for (batch_idx, chunk) in all_apps.chunks_mut(icon_batch_size).enumerate() {
            for app in chunk.iter_mut() {
                if !app.display_icon.is_empty() {
                    app.icon_base64 = crate::system::icon::extract_icon_to_base64(&app.display_icon);
                }
                if app.icon_base64.is_empty() {
                    if let Some(fallback) = find_fallback_exe(&app.install_location) {
                        app.icon_base64 = crate::system::icon::extract_icon_to_base64(&fallback);
                        if !app.icon_base64.is_empty() {
                            app.display_icon = fallback;
                        }
                    }
                }
            }

            // 收集本批有效图标推送给前端
            let updates: Vec<IconUpdate> = chunk
                .iter()
                .filter(|app| !app.icon_base64.is_empty())
                .map(|app| IconUpdate {
                    install_location: app.install_location.clone(),
                    icon_base64: app.icon_base64.clone(),
                })
                .collect();

            if !updates.is_empty() {
                let _ = app_handle.emit("scan-progress", ScanProgressEvent {
                    phase: "icons".to_string(),
                    apps: vec![],
                    icon_updates: updates,
                    total_count: total_app_count,
                    is_final: false,
                });
            }

            orbit_log!(
                "DEBUG", "scanner",
                "图标批次 {}/{} 完成",
                batch_idx + 1,
                total_batches
            );
        }

        // ── 写入内存缓存 ────────────────────────────────────────────────
        if let Ok(mut cache) = self.registry_cache.lock() {
            *cache = Some((Instant::now(), all_apps.clone()));
        }
        *self.last_full_scan.lock().unwrap() = Some(Instant::now());

        // ── 完成事件 ────────────────────────────────────────────────────
        let _ = app_handle.emit("scan-progress", ScanProgressEvent {
            phase: "done".to_string(),
            apps: vec![],
            icon_updates: vec![],
            total_count: all_apps.len(),
            is_final: true,
        });

        orbit_log!("INFO", "scanner", "流式扫描完成: {} 个应用, 总耗时 {}ms", all_apps.len(), total_start.elapsed().as_millis());
        Ok(all_apps)
    }

    /// 增量扫描：仅重新扫描注册表（若 TTL 过期），保留 Tier2/3 缓存
    #[allow(dead_code)]
    pub fn scan_incremental(&self) -> Result<Vec<InstalledApp>, String> {
        // 命中缓存则直接返回
        if let Ok(cache) = self.registry_cache.lock() {
            if let Some((timestamp, cached)) = cache.as_ref() {
                if timestamp.elapsed().as_secs() < REGISTRY_CACHE_TTL_SECS {
                    return Ok(cached.clone());
                }
            }
        }
        self.scan_all()
    }

    /// Tier 1：深度注册表解析
    #[cfg(windows)]
    fn scan_registry_deep(&self) -> Result<Vec<InstalledApp>, String> {
        let mut apps: Vec<InstalledApp> = Vec::new();

        let registry_paths: [(HKEY, &str, &str); 4] = [
            (HKEY_LOCAL_MACHINE, r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall", "HKLM"),
            (HKEY_LOCAL_MACHINE, r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall", "HKLM"),
            (HKEY_CURRENT_USER, r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall", "HKCU"),
            (HKEY_CURRENT_USER, r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall", "HKCU"),
        ];

        // 并行读取 4 个注册表路径
        let results: Vec<Vec<InstalledApp>> = registry_paths
            .par_iter()
            .filter_map(|(hkey, base_path, hive_name)| {
                self.scan_registry_path(*hkey, base_path, hive_name).ok()
            })
            .collect();

        for mut result in results {
            apps.append(&mut result);
        }

        // 按"名称+路径"去重
        let mut seen: HashSet<(String, String)> = HashSet::new();
        apps.retain(|app| {
            seen.insert((app.display_name.clone(), normalize_path(&app.install_location)))
        });

        Ok(apps)
    }

    /// 扫描单个注册表路径
    #[cfg(windows)]
    fn scan_registry_path(&self, hkey: HKEY, base_path: &str, hive_name: &str) -> Result<Vec<InstalledApp>, String> {
        let mut apps: Vec<InstalledApp> = Vec::new();
        let uninstall_key = RegKey::predef(hkey)
            .open_subkey(base_path)
            .map_err(|e| format!("打开注册表路径失败 {}: {}", base_path, e))?;

        for subkey_name in uninstall_key.enum_keys().filter_map(|k| k.ok()) {
            let subkey = match uninstall_key.open_subkey(&subkey_name) {
                Ok(k) => k,
                Err(_) => continue,
            };

            let display_name: String = subkey.get_value("DisplayName").unwrap_or_default();
            if display_name.is_empty() {
                continue;
            }

            // 系统组件过滤：KB 补丁、安全更新等
            if is_system_component(&display_name) {
                continue;
            }

            // 解析安装位置：三路汇聚 InstallLocation → DisplayIcon → UninstallString
            let install_location =
                resolve_install_location_from_registry(&subkey);

            if install_location.is_empty() {
                continue;
            }
            // UWP/MSIX 包：InstallLocation 在 \WindowsApps\ 下，系统管控不可迁移
            let loc_lower = install_location.to_lowercase();
            if loc_lower.contains("\\windowsapps\\")
                || loc_lower.contains("\\program files\\windowsapps")
                || loc_lower.contains("\\program files (x86)\\windowsapps")
            {
                continue;
            }
            // exists() 会跟随重解析点查询目标属性；目录联接（迁移后）若目标存在则通过
            // 若返回 false 再通过 symlink_metadata 确认是否为联接本身存在但目标不可达的情况
            let install_path = Path::new(&install_location);
            if !install_path.exists() {
                let is_symlink = install_path.symlink_metadata()
                    .map(|m| m.file_type().is_symlink())
                    .unwrap_or(false);
                if !is_symlink {
                    continue;
                }
                // 联接路径本身存在但目标暂不可达，保留该条目以便前端展示"已迁移"
            }

            let display_icon: String = subkey.get_value("DisplayIcon").unwrap_or_default();
            let publisher: String = subkey.get_value("Publisher").unwrap_or_default();
            let estimated_size: u64 =
                subkey.get_value::<u32, _>("EstimatedSize").unwrap_or(0) as u64;

            // DisplayIcon 校验：若指向的文件不存在则清空，由 extract_icons_parallel 兜底
            let effective_icon = validate_display_icon(&display_icon);

            // 规范化安装路径：去除尾部分隔符，防止图标提取和路径比对异常
            let install_location = install_location.trim_end_matches(['\\', '/']).to_string();

            let registry_path = format!("{}\\{}\\{}", hive_name, base_path, subkey_name);
            let icon_path = if effective_icon.is_empty() {
                String::new() // 清空无效路径，后续从安装目录搜索 exe 提取图标
            } else {
                effective_icon
            };

            apps.push(InstalledApp {
                display_name,
                install_location,
                display_icon: icon_path,
                estimated_size,
                icon_base64: String::new(),
                icon_url: String::new(),
                registry_path,
                publisher,
            });
        }

        Ok(apps)
    }

    /// Tier 2：LNK 快捷方式解析
    #[cfg(windows)]
    fn scan_lnk_shortcuts(&self, existing_paths: &HashSet<String>) -> Vec<InstalledApp> {
        let lnk_dirs = collect_lnk_search_dirs();
        let mut apps: Vec<InstalledApp> = Vec::new();
        let mut seen: HashSet<String> = existing_paths.clone();

        // 并行扫描各 LNK 目录（深度 5，覆盖 Programs\Tencent\WeChat\ 等嵌套）
        let results: Vec<Vec<InstalledApp>> = lnk_dirs
            .par_iter()
            .map(|dir| self.scan_lnk_dir(dir, 0, 5, &seen))
            .collect();

        for result in results {
            for app in result {
                let key = normalize_path(&app.install_location);
                if !seen.contains(&key) {
                    seen.insert(key);
                    apps.push(app);
                }
            }
        }

        apps
    }

    /// 递归扫描目录下的 .lnk 文件，深度上限 5
    /// 覆盖 Programs\Tencent\WeChat\WeChat.lnk 等深层嵌套快捷方式
    #[cfg(windows)]
    fn scan_lnk_dir(&self, dir: &Path, depth: usize, max_depth: usize, existing: &HashSet<String>) -> Vec<InstalledApp> {
        let mut apps: Vec<InstalledApp> = Vec::new();
        if depth > max_depth || !dir.exists() {
            return apps;
        }

        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return apps,
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                // 递归进入子目录（不限深度地探索 Start Menu 嵌套结构）
                apps.extend(self.scan_lnk_dir(&path, depth + 1, max_depth, existing));
                continue;
            }
            if path.extension().map(|e| e == "lnk").unwrap_or(false) {
                if let Some(app) = self.resolve_lnk_file(&path, existing) {
                    apps.push(app);
                }
            }
        }

        apps
    }

    /// 解析单个 .lnk 文件，提取目标 exe 路径和工作目录
    #[cfg(windows)]
    fn resolve_lnk_file(&self, lnk_path: &Path, existing: &HashSet<String>) -> Option<InstalledApp> {
        let target = parse_lnk_target(lnk_path)?;
        let target_path = Path::new(&target);

        // 只关注 .exe 目标
        if target_path.extension().map(|e| e != "exe").unwrap_or(true) {
            return None;
        }
        if !target_path.exists() {
            return None;
        }

        // 硬过滤：LNK 指向安装包/更新程序/卸载器 → 直接跳过
        let exe_name_lower = target_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_lowercase();
        if is_installer_like_exe(&exe_name_lower) {
            return None;
        }

        let dir_path = target_path.parent()?.to_path_buf();
        let install_location = dir_path.to_string_lossy().to_string();

        // 去重：已知路径跳过
        if existing.contains(&normalize_path(&install_location)) {
            return None;
        }

        // 跳过系统目录中的 exe
        if is_system_path(&install_location) {
            return None;
        }

        let display_name = lnk_path
            .file_stem()
            .and_then(|s| s.to_str())
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
            .unwrap_or_else(|| {
                target_path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("")
                    .to_string()
            });

        if display_name.is_empty() {
            return None;
        }

        Some(InstalledApp {
            display_name,
            install_location,
            display_icon: target_path.to_string_lossy().to_string(),
            estimated_size: 0,
            icon_base64: String::new(),
            icon_url: String::new(),
            registry_path: String::new(),
            publisher: String::new(),
        })
    }

    /// Tier 3：受限文件系统扫描
    #[cfg(windows)]
    fn scan_filesystem_constrained(&self, existing_paths: &HashSet<String>) -> Vec<InstalledApp> {
        let (pf_roots, lad_roots, other_roots, hp_roots) = collect_filesystem_roots();
        let mut apps: Vec<InstalledApp> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();

        // Program Files 系：深度 2（标准安装位置）
        let pf_results: Vec<InstalledApp> = pf_roots
            .par_iter()
            .flat_map(|root| {
                let mut out = Vec::new();
                let mut s = HashSet::new();
                scan_directory_constrained(root, 0, 2, existing_paths, &mut s, &mut out, None);
                out
            })
            .collect();

        // LocalAppData / ProgramData 系：深度 2
        let lad_results: Vec<InstalledApp> = lad_roots
            .par_iter()
            .flat_map(|root| {
                let mut out = Vec::new();
                let mut s = HashSet::new();
                scan_directory_constrained(root, 0, 2, existing_paths, &mut s, &mut out, None);
                out
            })
            .collect();

        // 高优先级自定义目录（D:\software, E:\tools 等）：深度 3
        let hp_results: Vec<InstalledApp> = hp_roots
            .par_iter()
            .flat_map(|root| {
                let mut out = Vec::new();
                let mut s = HashSet::new();
                scan_directory_constrained(root, 0, 3, existing_paths, &mut s, &mut out, None);
                out
            })
            .collect();

        // 非系统盘根目录：深度 2（严格分级，依赖注册表 + LNK 覆盖 99% 场景）
        let other_results: Vec<InstalledApp> = other_roots
            .par_iter()
            .flat_map(|root| {
                let mut out = Vec::new();
                let mut s = HashSet::new();
                scan_directory_constrained(root, 0, 2, existing_paths, &mut s, &mut out, None);
                out
            })
            .collect();

        for app in pf_results
            .into_iter()
            .chain(lad_results)
            .chain(hp_results)
            .chain(other_results)
        {
            let key = normalize_path(&app.install_location);
            if !seen.contains(&key) && !existing_paths.contains(&key) {
                seen.insert(key.clone());
                apps.push(app);
            }
        }

        apps
    }

    /// 并行提取图标，主路径失败时从安装目录搜索 exe 兜底
    #[cfg(windows)]
    fn extract_icons_parallel(&self, apps: &mut [InstalledApp]) {
        apps.par_iter_mut().for_each(|app| {
            // 主路径提取（DisplayIcon 指向的 exe/dll/ico）
            if !app.display_icon.is_empty() {
                app.icon_base64 =
                    crate::system::icon::extract_icon_to_base64(&app.display_icon);
            }
            // 兜底：主路径提取失败时，从安装目录搜索 exe 提取嵌入图标
            if app.icon_base64.is_empty() {
                if let Some(fallback) = find_fallback_exe(&app.install_location) {
                    app.icon_base64 =
                        crate::system::icon::extract_icon_to_base64(&fallback);
                    if !app.icon_base64.is_empty() {
                        app.display_icon = fallback;
                    }
                }
            }
        });
    }

    #[cfg(not(windows))]
    fn scan_registry_deep(&self) -> Result<Vec<InstalledApp>, String> {
        Ok(Vec::new())
    }
    #[cfg(not(windows))]
    fn scan_lnk_shortcuts(&self, _existing: &HashSet<String>) -> Vec<InstalledApp> {
        Vec::new()
    }
    #[cfg(not(windows))]
    fn scan_filesystem_constrained(&self, _existing: &HashSet<String>) -> Vec<InstalledApp> {
        Vec::new()
    }
    #[cfg(not(windows))]
    fn extract_icons_parallel(&self, _apps: &mut [InstalledApp]) {}
}

lazy_static::lazy_static! {
    /// 全局扫描器单例
    pub static ref SCANNER: AppScanner = AppScanner::new();
}

// ============================================================================
// 工具函数
// ============================================================================

/// 规范化路径：去除末尾分隔符、转小写
fn normalize_path(path: &str) -> String {
    let trimmed = path.trim().trim_matches('"');
    let without_tail = trimmed.trim_end_matches(['\\', '/']);
    without_tail.to_lowercase()
}

/// 从注册表子键汇聚安装位置：InstallLocation → DisplayIcon → UninstallString
#[cfg(windows)]
fn resolve_install_location_from_registry(subkey: &RegKey) -> String {
    // 1) InstallLocation
    let raw: String = subkey.get_value("InstallLocation").unwrap_or_default();
    let loc = raw.trim().trim_matches('"').trim_end_matches(['\\', '/']).to_string();
    if !loc.is_empty() {
        return loc;
    }

    // 2) DisplayIcon 推导
    let display_icon: String = subkey.get_value("DisplayIcon").unwrap_or_default();
    if let Some(dir) = derive_install_location_from_icon(&display_icon) {
        return dir.trim_end_matches(['\\', '/']).to_string();
    }

    // 3) UninstallString 推导
    let uninstall_string: String = subkey.get_value("UninstallString").unwrap_or_default();
    if let Some(dir) = derive_install_location_from_icon(&uninstall_string) {
        return dir.trim_end_matches(['\\', '/']).to_string();
    }

    String::new()
}

/// 校验 DisplayIcon 指向的文件是否存在，不存在则返回空
#[cfg(windows)]
fn validate_display_icon(display_icon: &str) -> String {
    if display_icon.is_empty() {
        return String::new();
    }
    let icon_file = display_icon
        .split(',')
        .next()
        .unwrap_or(display_icon)
        .trim()
        .trim_matches('"');
    if !icon_file.is_empty() && !Path::new(icon_file).exists() {
        orbit_log!(
            "DEBUG", "scanner",
            "DisplayIcon 缺失: {}, 保留应用但清空图标", icon_file
        );
        return String::new();
    }
    display_icon.to_string()
}

/// 在安装目录及其一级子目录中查找可提取图标的 exe 文件
/// 当 DisplayIcon 指向的 .ico 文件不存在或 exe 路径失效时兜底
#[cfg(windows)]
fn find_fallback_exe(install_location: &str) -> Option<String> {
    let dir = Path::new(install_location);
    if !dir.is_dir() {
        return None;
    }
    // 先查目录根下的 exe（如 D:\app\app.exe）
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file()
                && path.extension().map(|e| e.eq_ignore_ascii_case("exe")).unwrap_or(false)
            {
                return Some(path.to_string_lossy().to_string());
            }
        }
    }
    // 再查一级子目录（如 D:\app\bin\studio.exe）
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let sub_dir = entry.path();
            if !sub_dir.is_dir() {
                continue;
            }
            if let Ok(sub_entries) = std::fs::read_dir(&sub_dir) {
                for sub_entry in sub_entries.flatten() {
                    let sub_path = sub_entry.path();
                    if sub_path.is_file()
                        && sub_path.extension().map(|e| e.eq_ignore_ascii_case("exe")).unwrap_or(false)
                    {
                        return Some(sub_path.to_string_lossy().to_string());
                    }
                }
            }
        }
    }
    None
}

/// 从命令字符串中正则提取目录路径（兜底方案）
/// 处理 "C:\...\uninst.exe" /SILENT 等格式，即使 exe 已不存在也能提取目录
#[cfg(windows)]
fn extract_dir_from_command_string(raw: &str) -> Option<String> {
    let s = raw.trim().trim_matches('"');
    // 找第一个 X:\ 驱动器模式
    if let Some(drive_idx) = s.find(|c: char| c.is_ascii_alphabetic()) {
        let rest = &s[drive_idx..];
        if rest.len() < 3 || rest.as_bytes().get(1) != Some(&b':') || rest.as_bytes().get(2) != Some(&b'\\') {
            return None;
        }
        // 找路径结束：遇到 " 或空格后跟 / -
        let path_end = rest.find('"')
            .or_else(|| rest.find(" /"))
            .or_else(|| rest.find(" -"))
            .unwrap_or(rest.len());
        let path_str = &rest[..path_end].trim();
        let p = Path::new(path_str);
        // 获取父目录（若路径指向文件）或目录本身
        let dir = if p.is_file() || p.extension().is_some() {
            p.parent()?.to_path_buf()
        } else {
            p.to_path_buf()
        };
        let lower = dir.to_string_lossy().to_lowercase();
        if lower.contains("\\windows\\system32")
            || lower.contains("\\windows\\syswow64")
            || lower.contains("\\common files\\")
        {
            return None;
        }
        if dir.exists() {
            return Some(dir.to_string_lossy().trim_end_matches(['\\', '/']).to_string());
        }
    }
    None
}

/// 从 DisplayIcon / UninstallString 尝试推导安装目录
#[cfg(windows)]
fn derive_install_location_from_icon(icon_or_uninstall: &str) -> Option<String> {
    let raw = icon_or_uninstall.trim();
    if raw.is_empty() {
        return None;
    }
    let (before_comma, _) = raw.split_once(',').unwrap_or((raw, ""));
    let before_comma = before_comma.trim();

    let candidate = if before_comma.starts_with('"') {
        before_comma.trim_matches('"').to_string()
    } else {
        let tokens: Vec<&str> = before_comma.split_whitespace().collect();
        let mut found = None;
        for i in (1..=tokens.len()).rev() {
            let joined = tokens[..i].join(" ");
            if Path::new(&joined).exists() {
                found = Some(joined);
                break;
            }
        }
        found?
    };

    let p = Path::new(&candidate);
    if !p.exists() {
        // 正则兜底：exe 文件不存在时，尝试从命令字符串中提取目录
        // 如 "C:\App\unins000.exe" /SILENT → 提取 C:\App
        return extract_dir_from_command_string(raw);
    }
    let dir = if p.is_file() {
        p.parent()?.to_path_buf()
    } else {
        p.to_path_buf()
    };
    let lower = dir.to_string_lossy().to_lowercase();
    if lower.contains("\\windows\\system32")
        || lower.contains("\\windows\\syswow64")
        || lower.contains("\\common files\\")
    {
        return None;
    }
    Some(dir.to_string_lossy().trim_end_matches(['\\', '/']).to_string())
}

/// 判断文件名是否像安装包/卸载器/更新器
#[cfg(windows)]
fn is_installer_like_exe(file_name_lower: &str) -> bool {
    // 基础黑名单：安装包/更新器/卸载器关键字
    if file_name_lower.contains("setup")
        || file_name_lower.contains("install")
        || file_name_lower.contains("update")
        || file_name_lower.contains("upgrader")
        || file_name_lower.starts_with("unins")
        || file_name_lower.contains("uninst")
    {
        return true;
    }
    
    // 识别带版本号的安装包 (如 PCQQ2021.exe, v1.2.3_full.exe)
    // 应用主程序通常不包含 4 位连续数字（年份）或过长的数字串
    let has_year_pattern = file_name_lower.chars()
        .collect::<Vec<_>>()
        .windows(4)
        .any(|w| w.iter().all(|c| c.is_ascii_digit()));

    // 数字.数字 版本号模式（如 aDrive-6.9.1.exe、app-2.0.3.exe）
    let has_dot_version = file_name_lower
        .chars()
        .collect::<Vec<_>>()
        .windows(3)
        .any(|w| w[0].is_ascii_digit() && w[1] == '.' && w[2].is_ascii_digit());

    // 常见的安装包特征后缀
    if file_name_lower.ends_with("_x64.exe")
        || file_name_lower.ends_with("_x86.exe")
        || file_name_lower.ends_with(".msi")
    {
        return true;
    }

    // 如果文件名包含年份且长度较长，极大概率是安装包而非主程序
    if has_year_pattern && file_name_lower.len() > 10 {
        return true;
    }

    // 数字.数字 版本号在文件名中（不是目录名），安装包特征
    if has_dot_version {
        return true;
    }

    false
}

/// 判断 exe 文件名（不含扩展名）是否包含版本号模式
/// 如 "PCQQ2021"、"app_v1.2.3"、"setup_2024_x64"、"aDrive-6.9.1" 等
#[cfg(windows)]
fn has_version_pattern_in_stem(stem: &str) -> bool {
    let stem_lower = stem.to_lowercase();
    // 4 位连续数字（年份模式，如 2021、2024）
    let has_year = stem_lower
        .chars()
        .collect::<Vec<_>>()
        .windows(4)
        .any(|w| w.iter().all(|c| c.is_ascii_digit()));
    if has_year {
        return true;
    }
    // v 后跟数字版本号（v1、v1.2、v2.0.1）
    if let Some(v_pos) = stem_lower.find('v') {
        let after_v = &stem_lower[v_pos + 1..];
        if after_v.starts_with(|c: char| c.is_ascii_digit()) {
            return true;
        }
    }
    // 数字.数字 版本号模式（如 6.9.1、2.0、3.12.0）
    // 正常应用主程序极少在文件名中使用 X.Y 格式
    stem_lower
        .chars()
        .collect::<Vec<_>>()
        .windows(3)
        .any(|w| w[0].is_ascii_digit() && w[1] == '.' && w[2].is_ascii_digit())
}

/// 判断是否为开发/构建目录
#[cfg(windows)]
fn is_dev_directory(name: &str) -> bool {
    const DEV_DIRS: &[&str] = &[
        "node_modules", ".git", "target", "dist", "build",
        "__pycache__", ".venv", "venv", ".idea", ".vs",
        "vendor", "bower_components", ".cache", "obj",
        "debug", "release", "packages",
    ];
    let lower = name.to_lowercase();
    DEV_DIRS.iter().any(|d| &lower == d)
}

/// 判断是否为捆绑运行时目录
#[cfg(windows)]
fn is_bundled_runtime_dir(name: &str) -> bool {
    const RUNTIMES: &[&str] = &["jbr", "jre", "jdk", "rt", "gradle", "maven"];
    let lower = name.to_lowercase();
    RUNTIMES.iter().any(|r| &lower == r)
}

/// 综合判断应跳过的目录（开发目录、运行时、临时/下载目录）
#[cfg(windows)]
fn is_skippable_dir(name: &str) -> bool {
    // 常见临时与下载目录名——正常应用不会安装在这些目录下
    const TRANSIENT_DIRS: &[&str] = &[
        "download", "downloads", "temp", "tmp", "cache", "caches",
        "updater", "updates", "installation", "installers",
    ];
    let lower = name.to_lowercase();
    if TRANSIENT_DIRS.iter().any(|d| &lower == d) {
        return true;
    }
    is_dev_directory(name) || is_bundled_runtime_dir(name)
}

/// 判断子目录名是否为应用的支撑目录
#[cfg(windows)]
fn is_supporting_subdir(name: &str) -> bool {
    const SUPPORT_DIRS: &[&str] = &[
        "resources", "locales", "platforms", "translations",
        "data", "lib", "bin", "plugins", "modules",
        "languages", "help", "docs", "assets", "static",
        "config", "tools", "runtime", "scripts",
    ];
    let lower = name.to_lowercase();
    SUPPORT_DIRS.iter().any(|d| &lower == d)
}

// ============================================================================
// 噪声消减
// ============================================================================

/// 系统组件 DisplayName 模式匹配
/// 过滤 Windows Update 补丁、安全更新、语言包等非用户应用
#[cfg(windows)]
fn is_system_component(display_name: &str) -> bool {
    // KB 补丁号
    if display_name.starts_with("KB") && display_name.len() > 2 {
        return display_name[2..].chars().all(|c| c.is_ascii_digit());
    }
    let lower = display_name.to_lowercase();
    lower.contains("update for")
        || lower.contains("security update")
        || lower.contains("hotfix")
        || lower.contains("language pack")
        || lower.contains("service pack")
        || lower.starts_with("microsoft .net")
        || lower.starts_with("microsoft visual c++")
        // UWP/MSIX 运行时框架包，InstallLocation 在 \WindowsApps\ 下，用户不可管理
        || lower.contains("deploymentagent")
        || lower.contains("darkmodecheck")
        || lower.starts_with("microsoft.windowsappruntime")
        || lower.starts_with("microsoft.ui.xaml")
        || lower.starts_with("microsoft.vclibs")
        || lower.starts_with("microsoft.net.native")
}

/// 计算字符串的 Shannon 熵（用于检测随机文件名）
fn shannon_entropy(s: &str) -> f64 {
    if s.is_empty() {
        return 0.0;
    }
    let mut freq = [0u32; 256];
    let mut total = 0u32;
    for b in s.bytes() {
        freq[b as usize] += 1;
        total += 1;
    }
    let mut entropy = 0.0;
    for &count in freq.iter() {
        if count > 0 {
            let p = count as f64 / total as f64;
            entropy -= p * p.log2();
        }
    }
    entropy
}

/// 判断文件名是否为随机哈希（高熵）
fn is_random_filename(name: &str) -> bool {
    let stem = name
        .rfind('.')
        .map(|i| &name[..i])
        .unwrap_or(name);
    stem.len() >= 8 && shannon_entropy(stem) >= ENTROPY_THRESHOLD
}

/// 路径黑名单检查（扩展版）
#[cfg(windows)]
fn is_blacklisted_path(path: &Path) -> bool {
    // 基础黑名单
    const CORE_BLACKLIST: &[&str] = &[
        "windows",
        "$recycle.bin",
        "system volume information",
    ];
    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
        let nl = name.to_lowercase();
        if CORE_BLACKLIST.iter().any(|b| &nl == b) {
            return true;
        }
    }
    let lower = path.to_string_lossy().to_lowercase();
    if lower.ends_with("\\windows.old") {
        return true;
    }

    // 扩展黑名单：已知非应用目录
    const EXTENDED_BLACKLIST: &[&str] = &[
        "\\windows\\temp",
        "\\windows\\winsxs",
        "\\windows\\servicing",
        "\\windows\\softwaredistribution",
        "\\programdata\\package cache",
        "\\program files\\common files",
        "\\program files (x86)\\common files",
        "\\program files\\dotnet",
        "\\program files (x86)\\dotnet",
        // UWP/MSIX 包目录，系统管控，不可迁移
        "\\program files\\windowsapps",
        "\\program files (x86)\\windowsapps",
        "\\windowsapps\\",
        // Windows 运行时框架（WinUI、VCLibs 等）
        "\\program files\\modifiablewindowsapps",
    ];
    if EXTENDED_BLACKLIST.iter().any(|p| lower.contains(p)) {
        return true;
    }

    // TEMP 目录
    if let Ok(temp) = std::env::var("TEMP") {
        if lower.starts_with(&normalize_path(&temp)) {
            return true;
        }
    }

    false
}

/// 判断路径是否属于系统目录（用于 LNK 解析过滤）
fn is_system_path(path: &str) -> bool {
    let lower = path.to_lowercase();
    lower.contains("\\windows\\system32")
        || lower.contains("\\windows\\syswow64")
        || lower.contains("\\windows\\systemapps")
        || lower.contains("\\program files\\windowsapps")
        || lower.contains("\\program files (x86)\\windowsapps")
}

// ============================================================================
// 应用候选与评分（exe 驱动模型）
// ============================================================================

struct ApplicationCandidate {
    exe_path: PathBuf,
    exe_name: String,
    has_dll: bool,
    has_config: bool,
    has_supporting_subdirs: bool,
    exe_count: u32,
}

#[derive(Debug, Clone, Copy)]
enum NameMatchKind {
    Exact,
    Contains,
    None,
}

/// 多信号融合评分（0.0 ~ 1.0）
#[cfg(windows)]
fn score_application_candidate(
    exe_path: &Path,
    has_dll: bool,
    has_config: bool,
    has_supporting_subdirs: bool,
    exe_count: u32,
    name_match: NameMatchKind,
) -> f32 {
    let mut score: f32 = 0.0;

    // 基础分：exe 存在即为应用的有力证据
    score += 0.30;

    // 路径语义：不在下载目录
    let exe_lower = exe_path.to_string_lossy().to_lowercase();
    let in_downloads = (*DOWNLOADS_DIR_LOWER)
        .as_ref()
        .map(|dl| exe_lower.starts_with(dl.as_str()))
        .unwrap_or(false);
    if !in_downloads {
        score += 0.10;
    }

    if has_dll {
        score += 0.15;
    }
    if has_config {
        score += 0.10;
    }
    if has_supporting_subdirs {
        score += 0.10;
    }
    if exe_count >= 2 {
        score += 0.05;
    }

    match name_match {
        NameMatchKind::Exact => score += 0.35,
        NameMatchKind::Contains => score += 0.25,
        NameMatchKind::None => {}
    }

    score = score.min(1.0);

    // 提前提取 stem（供后续多项检查复用）
    let stem = exe_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("");

    // 版本号模式惩罚（无论名称是否匹配均适用）
    // 阻止 aDrive-6.9.1.exe 等安装包因 Exact name match 获得过高评分
    if has_version_pattern_in_stem(stem) {
        score -= 0.30;
    }

    // 随机文件名惩罚（高熵）
    if is_random_filename(stem) {
        score -= 0.20;
    }

    // 数字占比惩罚：exe 名含大量数字但与父目录名不匹配（如 PCQQ2021.exe）
    if matches!(name_match, NameMatchKind::None) {
        let digit_count = stem.chars().filter(|c| c.is_ascii_digit()).count();
        if stem.len() > 0 {
            let digit_ratio = digit_count as f32 / stem.len() as f32;
            if digit_ratio > 0.30 {
                score -= 0.15;
            }
        }
    }

    score
}

/// exe 驱动目录识别：扫描目录，对每个 exe 独立评分，返回最佳候选
#[cfg(windows)]
fn directory_looks_like_app(dir: &Path) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    let mut candidates: Vec<ApplicationCandidate> = Vec::new();
    let mut best_launcher: Option<PathBuf> = None;
    let mut has_non_installer_exe = false;
    let mut has_dll = false;
    let mut has_config = false;
    let mut has_supporting_subdirs = false;
    let mut exe_count: u32 = 0;
    let dir_name_lower = dir
        .file_name()
        .map(|n| n.to_string_lossy().to_lowercase())
        .unwrap_or_default();

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if is_supporting_subdir(name) {
                    has_supporting_subdirs = true;
                }
            }
            continue;
        }
        if !path.is_file() {
            continue;
        }
        let file_name_lower = path
            .file_name()
            .map(|n| n.to_string_lossy().to_lowercase())
            .unwrap_or_default();
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            match ext.to_lowercase().as_str() {
                "exe" => {
                    exe_count += 1;
                    // 安装包/更新程序一票否决，不进入候选列表
                    if is_installer_like_exe(&file_name_lower) {
                        continue;
                    }
                    has_non_installer_exe = true;
                    let exe_name = path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .filter(|s| !s.is_empty())
                        .map(|s| s.to_string())
                        .unwrap_or_default();
                    // 跳过无意义文件名 + 随机哈希文件名
                    if exe_name.len() <= 1 || is_random_filename(&exe_name) {
                        continue;
                    }
                    candidates.push(ApplicationCandidate {
                        exe_path: path.clone(),
                        exe_name,
                        has_dll: false,
                        has_config: false,
                        has_supporting_subdirs: false,
                        exe_count: 0,
                    });
                }
                "dll" => has_dll = true,
                "bat" | "cmd" => {
                    if best_launcher.is_none() {
                        best_launcher = Some(path);
                    }
                }
                "ini" | "xml" | "json" | "cfg" | "conf" | "toml" | "yaml" | "yml" => {
                    has_config = true;
                }
                _ => {}
            }
        }
    }

    // 阶段1：纯安装包目录过滤
    let is_pure_installer_dir = !has_non_installer_exe
        && best_launcher.is_none()
        && !has_dll
        && !has_config
        && !has_supporting_subdirs
        && exe_count > 0;

    if is_pure_installer_dir {
        return None;
    }

    // 排除下载/临时目录中无旁证的 exe
    let in_transient_dir = dir_name_lower == "download"
        || dir_name_lower == "downloads"
        || dir_name_lower == "temp"
        || dir_name_lower == "tmp";
    if in_transient_dir && !has_dll && !has_config && !has_supporting_subdirs {
        return None;
    }

    // 回填共享信号
    for c in &mut candidates {
        c.has_dll = has_dll;
        c.has_config = has_config;
        c.has_supporting_subdirs = has_supporting_subdirs;
        c.exe_count = exe_count;
    }

    // 阶段2：评分选取最佳 exe
    let mut best_exe: Option<PathBuf> = None;
    let mut best_score: f32 = 0.0;

    for c in &candidates {
        let exe_name_lower = c.exe_name.to_lowercase();
        let name_match = if exe_name_lower == dir_name_lower {
            NameMatchKind::Exact
        } else if !exe_name_lower.is_empty()
            && !dir_name_lower.is_empty()
            && (dir_name_lower.contains(&exe_name_lower) || exe_name_lower.contains(&dir_name_lower))
        {
            NameMatchKind::Contains
        } else {
            NameMatchKind::None
        };

        let score = score_application_candidate(
            &c.exe_path,
            c.has_dll,
            c.has_config,
            c.has_supporting_subdirs,
            c.exe_count,
            name_match,
        );

        if score > best_score {
            best_score = score;
            best_exe = Some(c.exe_path.clone());
        }
    }

    if best_score >= SCORE_THRESHOLD {
        return best_exe;
    }

    None
}

// ============================================================================
// Tier 2 支撑：LNK 解析 + 搜索目录
// ============================================================================

/// 收集所有需要扫描 .lnk 文件的系统目录
#[cfg(windows)]
fn collect_lnk_search_dirs() -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();

    // %APPDATA%\Microsoft\Windows\Start Menu\Programs
    if let Ok(appdata) = std::env::var("APPDATA") {
        let p = PathBuf::from(&appdata)
            .join("Microsoft")
            .join("Windows")
            .join("Start Menu")
            .join("Programs");
        dirs.push(p);
    }

    // %PROGRAMDATA%\Microsoft\Windows\Start Menu\Programs
    if let Ok(pd) = std::env::var("PROGRAMDATA") {
        let p = PathBuf::from(&pd)
            .join("Microsoft")
            .join("Windows")
            .join("Start Menu")
            .join("Programs");
        dirs.push(p);
    }

    // Desktop (user)
    if let Some(desktop) = dirs::desktop_dir() {
        dirs.push(desktop);
    }

    // Public Desktop
    if let Ok(pd) = std::env::var("PUBLIC") {
        dirs.push(PathBuf::from(&pd).join("Desktop"));
    }

    dirs
}

/// 手动解析 LNK 文件，提取目标路径
///
/// LNK 二进制格式（简化解析，仅提取目标路径）：
/// - 偏移 0x00: 4 字节 GUID = {00021401-0000-0000-C000-000000000046}
/// - 偏移 0x14: 4 字节 LinkFlags
///   - bit 1 (0x02): HasLinkInfo — 含 LocalBasePath
/// - 跳过 LinkTargetIDList（若 bit 0 置位）
/// - LinkInfo 结构中提取 LocalBasePath 字符串
#[cfg(windows)]
fn parse_lnk_target(lnk_path: &Path) -> Option<String> {
    let data = std::fs::read(lnk_path).ok()?;
    if data.len() < 76 {
        return None;
    }

    // 校验 GUID
    let guid: [u8; 16] = [
        0x01, 0x14, 0x02, 0x00, 0x00, 0x00, 0x00, 0x00,
        0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46,
    ];
    if data.len() < 16 || data[..16] != guid {
        return None;
    }

    // LinkFlags 在偏移 0x14
    let flags = u32::from_le_bytes([data[0x14], data[0x15], data[0x16], data[0x17]]);
    let has_link_target_id_list = (flags & 0x01) != 0;
    let has_link_info = (flags & 0x02) != 0;

    if !has_link_info {
        return None;
    }

    // 跳过 Header(76) + LinkTargetIDList
    let mut offset = 76usize;
    if has_link_target_id_list {
        if offset + 2 > data.len() {
            return None;
        }
        let id_list_size = u16::from_le_bytes([data[offset], data[offset + 1]]) as usize;
        offset += id_list_size;
    }

    // 跳过 LinkInfo header 到 LocalBasePath
    if offset + 20 > data.len() {
        return None;
    }
    let link_info_size = u32::from_le_bytes([
        data[offset], data[offset + 1], data[offset + 2], data[offset + 3],
    ]) as usize;
    if link_info_size < 16 || offset + link_info_size > data.len() {
        return None;
    }

    let link_info_flags = u32::from_le_bytes([
        data[offset + 8], data[offset + 9], data[offset + 10], data[offset + 11],
    ]);
    // VolumeIDAndLocalBasePath 位 (bit 0)
    let has_volume_and_local = (link_info_flags & 0x01) != 0;
    if !has_volume_and_local {
        return None;
    }

    let local_base_path_offset =
        u32::from_le_bytes([
            data[offset + 16], data[offset + 17], data[offset + 18], data[offset + 19],
        ]) as usize;

    let str_offset = offset + local_base_path_offset;
    if str_offset >= data.len() {
        return None;
    }

    // 读取 null-terminated string
    let mut end = str_offset;
    while end < data.len() && data[end] != 0 {
        end += 1;
    }
    let target_bytes = &data[str_offset..end];
    String::from_utf8(target_bytes.to_vec()).ok()
}

// ============================================================================
// Tier 3 支撑：文件系统扫描
// ============================================================================

/// 收集文件系统扫描根目录
/// 返回 (program_files, local_app_data, other_drives, high_priority_app_dirs)
#[cfg(windows)]
fn collect_filesystem_roots() -> (Vec<PathBuf>, Vec<PathBuf>, Vec<PathBuf>, Vec<PathBuf>) {
    let mut pf_roots: Vec<PathBuf> = Vec::new();
    if let Some(pf) = std::env::var_os("ProgramFiles") {
        pf_roots.push(PathBuf::from(pf));
    }
    if let Some(pf86) = std::env::var_os("ProgramFiles(x86)") {
        pf_roots.push(PathBuf::from(pf86));
    }

    let mut lad_roots: Vec<PathBuf> = Vec::new();
    if let Some(la) = std::env::var_os("LocalAppData") {
        let local_app_data = PathBuf::from(la);
        lad_roots.push(local_app_data.clone());
        let programs = local_app_data.join("Programs");
        if programs.exists() {
            lad_roots.push(programs);
        }
    }
    if let Some(pd) = std::env::var_os("ProgramData") {
        lad_roots.push(PathBuf::from(pd));
    }

    let mut other_roots: Vec<PathBuf> = Vec::new();
    let mut high_priority_roots: Vec<PathBuf> = Vec::new();

    // 用户常见的便携/绿色应用存放目录名
    const APP_DIR_NAMES: &[&str] = &[
        "software", "app", "apps", "tools", "games", "programs", "applications", "portable",
    ];

    let disks = sysinfo::Disks::new_with_refreshed_list();
    for disk in &disks {
        let mount = disk.mount_point();
        let mount_str = mount.to_string_lossy().to_uppercase();
        if mount_str.starts_with("C:") {
            continue;
        }
        let mount_path = mount.to_path_buf();
        other_roots.push(mount_path.clone());

        // 将用户常见的软件存放目录列为高优先级扫描根
        for dir_name in APP_DIR_NAMES {
            let candidate = mount_path.join(dir_name);
            if candidate.exists() && candidate.is_dir() {
                high_priority_roots.push(candidate);
            }
        }
    }

    (pf_roots, lad_roots, other_roots, high_priority_roots)
}

/// 受限递归扫描（单线程内部使用，由 rayon 并行调度外层）
#[cfg(windows)]
fn scan_directory_constrained(
    dir: &Path,
    depth: usize,
    max_depth: usize,
    existing_paths: &HashSet<String>,
    seen: &mut HashSet<String>,
    out: &mut Vec<InstalledApp>,
    registry_install_location: Option<&Path>,
) {
    if depth > max_depth {
        return;
    }
    if is_blacklisted_path(dir) {
        return;
    }
    if let Some(name) = dir.file_name().and_then(|n| n.to_str()) {
        if is_skippable_dir(name) {
            return;
        }
        if name.eq_ignore_ascii_case("InstallShield Installation Information") {
            return;
        }
    }

    // Quark 深度过滤：exe 目录深度超过 InstallLocation + 3 则跳过
    if let Some(reg_loc) = registry_install_location {
        if let Ok(relative) = dir.strip_prefix(reg_loc) {
            if relative.components().count() > 3 {
                return; // 辅助组件，非独立应用
            }
        }
    }

    if let Some(exe_path) = directory_looks_like_app(dir) {
        // Tier 3 激进过滤：exe 含版本号且不在注册表/LNK 已知路径中 → 丢弃
        // 阻止 PCQQ2021.exe、app_v2.0_setup.exe 等安装包被误判为应用
        let dir_key = normalize_path(&dir.to_string_lossy());
        if !existing_paths.contains(&dir_key) {
            let stem = exe_path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("");
            if has_version_pattern_in_stem(stem) {
                return;
            }
        }
        maybe_push_app(dir, &exe_path, existing_paths, seen, out);
        return;
    }

    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(ft) = entry.file_type() else {
            continue;
        };
        if !ft.is_dir() {
            continue;
        }
        if ft.is_symlink() {
            // 迁移后的目录联接仍需检查是否为可识别应用
            // 绿色软件（无注册表条目）仅靠 Tier3 发现，跳过会导致迁移后消失
            let symlink_dir = entry.path();
            if let Some(exe_path) = directory_looks_like_app(&symlink_dir) {
                let dir_key = normalize_path(&symlink_dir.to_string_lossy());
                if !existing_paths.contains(&dir_key) {
                    let stem = exe_path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
                    if !has_version_pattern_in_stem(stem) {
                        maybe_push_app(&symlink_dir, &exe_path, existing_paths, seen, out);
                        // 将联接目标物理路径也加入 seen，防止 Tier3 在新盘重复发现
                        if let Ok(target) = std::fs::read_link(&symlink_dir) {
                            seen.insert(normalize_path(&target.to_string_lossy()));
                        }
                    }
                }
            }
            continue; // 不递归进入联接内部，避免重复计算
        }
        scan_directory_constrained(
            &entry.path(),
            depth + 1,
            max_depth,
            existing_paths,
            seen,
            out,
            registry_install_location,
        );
    }
}

/// 将候选目录注册为应用
#[cfg(windows)]
fn maybe_push_app(
    dir: &Path,
    exe_path: &Path,
    existing_paths: &HashSet<String>,
    seen: &mut HashSet<String>,
    out: &mut Vec<InstalledApp>,
) {
    let install_location = dir.to_string_lossy().to_string();
    let loc_key = normalize_path(&install_location);
    let exe_key = normalize_path(&exe_path.to_string_lossy());
    if loc_key.is_empty()
        || exe_key.is_empty()
        || existing_paths.contains(&loc_key)
        || seen.contains(&loc_key)
        || seen.contains(&exe_key)
    {
        return;
    }

    let display_name = exe_path
        .file_stem()
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| {
            dir.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("")
                .to_string()
        });

    if display_name.is_empty() {
        return;
    }

    seen.insert(loc_key);
    seen.insert(exe_key);

    out.push(InstalledApp {
        display_name,
        install_location,
        display_icon: exe_path.to_string_lossy().to_string(),
        estimated_size: 0,
        icon_base64: String::new(),
        icon_url: String::new(),
        registry_path: String::new(),
        publisher: String::new(),
    });
}

// ============================================================================
// 后处理
// ============================================================================

/// 子目录去重：若 path_j 是 path_i 的子目录，移除 path_j
/// 排序后线性扫描 O(n log n)，替代原双层循环 O(n²)
#[cfg(windows)]
fn dedup_subdirectory_apps(apps: &mut Vec<InstalledApp>) {
    // 按路径字典序排序，子目录必然紧跟在父目录后面
    apps.sort_unstable_by(|a, b| {
        normalize_path(&a.install_location).cmp(&normalize_path(&b.install_location))
    });

    let paths: Vec<String> = apps
        .iter()
        .map(|a| normalize_path(&a.install_location))
        .collect();

    let mut remove_set: HashSet<usize> = HashSet::new();
    for i in 0..paths.len() {
        if remove_set.contains(&i) {
            continue;
        }
        for j in (i + 1)..paths.len() {
            if paths[j].starts_with(&paths[i])
                && paths[j].as_bytes().get(paths[i].len()) == Some(&b'\\')
            {
                remove_set.insert(j);
            } else if !paths[j].starts_with(&paths[i]) {
                // 排序后该父目录下不会再有更多子目录
                break;
            }
        }
    }

    if remove_set.is_empty() {
        return;
    }

    let mut idx = 0;
    apps.retain(|_| {
        let keep = !remove_set.contains(&idx);
        idx += 1;
        keep
    });
}

/// 计算目录下所有文件的总大小（KB）
#[cfg(windows)]
fn compute_dir_size_kb(dir: &Path) -> u64 {
    crate::utils::get_dir_size_safe(dir) / 1024
}

// ============================================================================
// 公共 API
// ============================================================================

/// 获取已安装应用列表（优先读取内存缓存，避免重复全量扫描）
pub fn get_installed_apps() -> Result<Vec<InstalledApp>, String> {
    crate::app_manager::cache::get_or_scan()
}

/// 增量扫描：仅刷新注册表（若 TTL 过期）
#[allow(dead_code)]
pub fn get_installed_apps_incremental() -> Result<Vec<InstalledApp>, String> {
    #[cfg(windows)]
    {
        SCANNER.scan_incremental()
    }
    #[cfg(not(windows))]
    {
        Ok(Vec::new())
    }
}

/// 按需获取应用目录大小（延迟计算，不阻塞主扫描流程）
pub fn get_app_size(install_location: String) -> Result<u64, String> {
    #[cfg(windows)]
    {
        let dir = Path::new(&install_location);
        // 迁移后路径为目录联接，exists() 跟随重解析点到目标
        // 若目标不可达但联接本身存在，仍允许计算大小（前端的容错逻辑会自动跳过）
        if !dir.exists() && !dir.is_symlink() {
            return Err(format!("目录不存在: {}", install_location));
        }
        if !dir.exists() && dir.is_symlink() {
            return Ok(0); // 联接目标暂时不可达，返回 0
        }
        Ok(compute_dir_size_kb(dir))
    }
    #[cfg(not(windows))]
    {
        Ok(0)
    }
}

/// 检测指定路径是否被进程占用
pub fn check_process_locks(source_path: String) -> Result<ProcessLockResult, String> {
    let source = Path::new(&source_path);

    if !source.exists() {
        return Err(format!("源路径不存在: {}", source_path));
    }

    let mut sys = System::new_all();
    sys.refresh_all();

    let mut locked_processes: Vec<String> = Vec::new();
    let source_lower = source_path.to_lowercase();

    for (_, process) in sys.processes() {
        if let Some(exe_path) = process.exe() {
            let exe_str = exe_path.to_string_lossy().to_lowercase();
            if exe_str.starts_with(&source_lower) {
                let name = process.name().to_string_lossy().to_string();
                if !locked_processes.contains(&name) {
                    locked_processes.push(name);
                }
            }
        }
    }

    Ok(ProcessLockResult {
        is_locked: !locked_processes.is_empty(),
        processes: locked_processes,
    })
}
