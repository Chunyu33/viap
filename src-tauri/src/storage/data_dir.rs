// 数据目录管理模块
//
// 架构说明：
// - 安装版指针文件位于 %APPDATA%/viap.json，便携版位于程序同级 viap.json
// - 安装版默认数据目录为 %APPDATA%/viap/，便携版为程序同级 data/
// - 用户可在设置中修改数据目录，数据文件自动迁移
// - 启动时检测数据目录是否存在，缺失则自动重建

use std::path::{Path, PathBuf};

use crate::models::{ConfigFileEntry, DataDirConfig, DataDirSwitchResult, CustomFolderEntry};

#[derive(Debug, serde::Serialize)]
pub struct StorageInitializationResult {
    pub data_dir: String,
    pub imported_legacy_data: bool,
    pub warning: Option<String>,
}

#[cfg(feature = "portable")]
fn portable_root_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(feature = "portable")]
fn config_file_path() -> PathBuf {
    // 便携版配置跟随程序目录，复制整个文件夹到新位置后仍能保留用户设置。
    portable_root_dir().join("viap.json")
}

#[cfg(not(feature = "portable"))]
fn config_file_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("viap.json")
}

#[cfg(feature = "portable")]
fn default_data_dir() -> PathBuf {
    // 便携版默认不写入 %APPDATA%，避免留下安装版痕迹并支持直接复制迁移。
    portable_root_dir().join("data")
}

#[cfg(feature = "portable")]
fn legacy_config_file_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("viap.json")
}

#[cfg(feature = "portable")]
fn legacy_default_data_dir() -> PathBuf {
    std::env::var("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("viap")
}

fn read_data_dir_config(path: &Path) -> Option<(PathBuf, bool)> {
    let contents = std::fs::read_to_string(path).ok()?;
    let config = serde_json::from_str::<DataDirConfig>(&contents).ok()?;
    if config.data_dir.trim().is_empty() {
        return None;
    }

    let configured_path = PathBuf::from(config.data_dir);
    #[cfg(feature = "portable")]
    if configured_path.is_relative() {
        return Some((portable_root_dir().join(configured_path), config.portable_default));
    }

    Some((configured_path, config.portable_default))
}

#[cfg(feature = "portable")]
fn portable_config_uses_moved_default(path: &Path) -> bool {
    let local_default = default_data_dir();
    let looks_like_default = path
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.eq_ignore_ascii_case("data"))
        .unwrap_or(false);

    // 旧版便携配置保存的是绝对 data 路径；程序目录被整体移动后，旧路径通常已不存在。
    // 仅对不存在的、末级名为 data 的路径修正，避免覆盖仍有效的自定义数据目录。
    looks_like_default && path != local_default && (!path.exists() || local_default.exists())
}

#[cfg(not(feature = "portable"))]
fn default_data_dir() -> PathBuf {
    // 安装版默认路径与旧版保持兼容，避免升级后丢失已有数据。
    let appdata = std::env::var("APPDATA").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(appdata).join("viap")
}

/// 获取指针文件路径
/// 获取记录实际数据目录的配置文件路径。
pub fn get_config_path() -> PathBuf {
    config_file_path()
}

/// 获取实际数据目录（读取指针文件 → 返回配置路径，或默认值）
pub fn get_data_dir() -> PathBuf {
    let config_path = get_config_path();
    if config_path.exists() {
        if let Some((path, portable_default)) = read_data_dir_config(&config_path) {
            #[cfg(feature = "portable")]
            if portable_default || portable_config_uses_moved_default(&path) {
                return default_data_dir();
            }
            #[cfg(not(feature = "portable"))]
            let _ = portable_default;
            return path;
        }
    }
    default_data_dir()
}

/// 确保数据目录存在，缺失则自动重建
pub fn ensure_data_dir() -> PathBuf {
    let dir = get_data_dir();
    if !dir.exists() {
        let _ = std::fs::create_dir_all(&dir);
    }
    dir
}

/// 获取当前数据目录信息（供前端设置页展示）
#[tauri::command]
pub fn get_data_dir_info() -> Result<DataDirConfig, String> {
    let dir = get_data_dir();
    Ok(DataDirConfig {
        data_dir: dir.to_string_lossy().to_string(),
        portable_default: cfg!(feature = "portable") && dir == default_data_dir(),
    })
}

/// 列出可展示的配置文件：数据目录指针文件与界面设置文件
///
/// 前端按 id 映射展示文案，这里只给出路径与存在性，避免把界面文案写进后端。
#[tauri::command]
pub fn get_config_files() -> Result<Vec<ConfigFileEntry>, String> {
    let pointer_path = get_config_path();
    let settings_path = crate::storage::user_settings::settings_path();

    Ok(vec![
        ConfigFileEntry {
            id: "pointer".to_string(),
            exists: pointer_path.exists(),
            path: pointer_path.to_string_lossy().to_string(),
        },
        ConfigFileEntry {
            id: "ui_settings".to_string(),
            exists: settings_path.exists(),
            path: settings_path.to_string_lossy().to_string(),
        },
    ])
}

fn comparable_path(path: &Path) -> PathBuf {
    if let Ok(canonical) = std::fs::canonicalize(path) {
        return canonical;
    }
    if path.is_absolute() {
        return path.to_path_buf();
    }
    std::env::current_dir()
        .map(|current| current.join(path))
        .unwrap_or_else(|_| path.to_path_buf())
}

fn paths_overlap(first: &Path, second: &Path) -> bool {
    let first = comparable_path(first);
    let second = comparable_path(second);
    let first_parts: Vec<String> = first
        .components()
        .map(|component| component.as_os_str().to_string_lossy().to_lowercase())
        .collect();
    let second_parts: Vec<String> = second
        .components()
        .map(|component| component.as_os_str().to_string_lossy().to_lowercase())
        .collect();
    is_component_prefix(&first_parts, &second_parts)
        || is_component_prefix(&second_parts, &first_parts)
}

fn is_component_prefix(parent: &[String], child: &[String]) -> bool {
    child.len() >= parent.len() && child.iter().zip(parent).all(|(left, right)| left == right)
}

/// 数据目录中由本程序管理的文件前缀（含 .bak / .tmp 变体）与目录名
///
/// 数据目录允许用户自行指定，很可能同时是迁移目标根目录（里面是几十 GB 的应用数据），
/// 因此搬迁数据目录时只能逐项处理这些受管条目，绝不能递归整个目录。
const MANAGED_FILE_STEMS: &[&str] = &[
    "migration_history",
    "migrated_apps",
    "custom_folders",
    "app_data_templates",
    "ui_settings",
    "uninstall_logs",
];
const MANAGED_DIRECTORY: &str = "cache";

/// 判断数据目录下的某个条目是否由本程序管理
fn is_managed_entry(file_name: &str, is_dir: bool) -> bool {
    let lower = file_name.to_lowercase();
    if is_dir {
        return lower == MANAGED_DIRECTORY;
    }
    MANAGED_FILE_STEMS
        .iter()
        .any(|stem| lower == format!("{}.json", stem) || lower.starts_with(&format!("{}.json.", stem)))
}

/// 判断目录是否确实是本程序的缓存目录
///
/// 数据目录可能同时被用户选作迁移目标根目录，里面可能存在同名 cache 目录。
/// 只有包含本程序的缓存文件时才按受管目录处理，避免误搬或误删应用数据。
fn looks_like_app_cache(path: &Path) -> bool {
    const CACHE_FILES: &[&str] = &["app_snapshot.json", "size_cache.json"];
    const CACHE_DIRS: &[&str] = &["icons"];

    CACHE_FILES.iter().any(|name| path.join(name).is_file())
        || CACHE_DIRS.iter().any(|name| path.join(name).is_dir())
}

/// 复制 Viap 的受管数据到新数据目录，返回复制的文件数
fn migrate_data_files(old_dir: &Path, new_dir: &Path, overwrite: bool) -> Result<u32, String> {
    if paths_overlap(old_dir, new_dir) {
        return Err("新数据目录不能位于旧数据目录内部或反过来".to_string());
    }

    std::fs::create_dir_all(new_dir)
        .map_err(|e| format!("无法创建数据目录: {}", e))?;

    if !old_dir.exists() {
        return Ok(0);
    }

    let cache_is_managed = looks_like_app_cache(&old_dir.join(MANAGED_DIRECTORY));
    let mut copied_files = 0u32;
    for entry in walkdir::WalkDir::new(old_dir).follow_links(false) {
        let entry = entry.map_err(|error| format!("读取数据目录失败: {}", error))?;
        let relative = entry
            .path()
            .strip_prefix(old_dir)
            .map_err(|error| format!("解析数据目录结构失败: {}", error))?;
        let mut components = relative.components();
        let Some(first_component) = components.next() else {
            continue;
        };
        let first_name = first_component.as_os_str().to_string_lossy().to_string();
        let is_top_level = components.next().is_none();

        // 只处理受管条目的顶层项：目录（cache）需要继续深入，其余顶层内容全部跳过
        if is_top_level {
            if !is_managed_entry(&first_name, entry.file_type().is_dir()) {
                continue;
            }
            if entry.file_type().is_dir() && !cache_is_managed {
                continue;
            }
        } else if !first_name.eq_ignore_ascii_case(MANAGED_DIRECTORY) || !cache_is_managed {
            continue;
        }

        // 临时文件可能只写入了一半，不能在新目录中恢复成有效配置。
        if entry.file_type().is_file()
            && entry.path().extension().and_then(|ext| ext.to_str()) == Some("tmp")
        {
            continue;
        }

        let target = new_dir.join(relative);
        if entry.file_type().is_dir() {
            std::fs::create_dir_all(&target)
                .map_err(|error| format!("创建数据目录失败 {}: {}", target.display(), error))?;
        } else if entry.file_type().is_file() && (overwrite || !target.exists()) {
            if let Some(parent) = target.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|error| format!("创建数据目录失败 {}: {}", parent.display(), error))?;
            }
            std::fs::copy(entry.path(), &target)
                .map_err(|error| format!("迁移数据文件失败 {}: {}", relative.display(), error))?;
            copied_files += 1;
        }
    }
    Ok(copied_files)
}

/// 删除旧数据目录中的受管数据，返回删除的顶层条目数
///
/// 只删除本程序命名的文件与 cache 目录；目录本身以及其它任何内容都不动，
/// 避免用户把数据目录选在迁移目标根目录时误删应用数据。
fn remove_managed_data_entries(old_dir: &Path) -> Result<u32, String> {
    if !old_dir.exists() {
        return Ok(0);
    }

    let entries = std::fs::read_dir(old_dir)
        .map_err(|error| format!("读取旧数据目录失败 {}: {}", old_dir.display(), error))?;

    let mut removed = 0u32;
    for entry in entries {
        let entry = entry.map_err(|error| format!("读取旧数据目录条目失败: {}", error))?;
        let file_name = entry.file_name().to_string_lossy().to_string();
        let file_type = entry
            .file_type()
            .map_err(|error| format!("读取条目类型失败 {}: {}", file_name, error))?;

        if !is_managed_entry(&file_name, file_type.is_dir()) {
            continue;
        }
        // cache 目录只在确认是本程序缓存时才删除，避免误删同名的应用数据
        if file_type.is_dir() && !looks_like_app_cache(&entry.path()) {
            continue;
        }

        let path = entry.path();
        let result = if file_type.is_dir() {
            std::fs::remove_dir_all(&path).map_err(|error| format!("删除 {} 失败: {}", path.display(), error))
        } else {
            std::fs::remove_file(&path).map_err(|error| format!("删除 {} 失败: {}", path.display(), error))
        };
        result?;
        removed += 1;
    }

    Ok(removed)
}

fn write_data_dir_config(data_dir: &Path) -> Result<(), String> {
    let config_path = get_config_path();
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("创建配置目录失败: {}", error))?;
    }

    let stored_path = {
        #[cfg(feature = "portable")]
        {
            if data_dir == default_data_dir() {
                PathBuf::from("data")
            } else {
                data_dir.to_path_buf()
            }
        }
        #[cfg(not(feature = "portable"))]
        {
            data_dir.to_path_buf()
        }
    };
    let config = DataDirConfig {
        data_dir: stored_path.to_string_lossy().to_string(),
        portable_default: cfg!(feature = "portable") && data_dir == default_data_dir(),
    };
    let json = serde_json::to_string_pretty(&config)
        .map_err(|error| format!("序列化配置失败: {}", error))?;
    let temp_config = config_path.with_extension("json.tmp");
    std::fs::write(&temp_config, json)
        .map_err(|error| format!("写入配置文件失败: {}", error))?;
    std::fs::rename(&temp_config, &config_path)
        .map_err(|error| format!("配置文件重命名失败: {}", error))?;
    Ok(())
}

#[cfg(feature = "portable")]
fn find_legacy_data_dir() -> PathBuf {
    read_data_dir_config(&legacy_config_file_path())
        .map(|(path, _)| path)
        .unwrap_or_else(legacy_default_data_dir)
}

/// 初始化存储根目录，并在便携版首次运行时导入安装版数据。
#[tauri::command]
pub fn initialize_storage() -> Result<StorageInitializationResult, String> {
    #[cfg(feature = "portable")]
    {
        let config_path = get_config_path();
        if !config_path.exists() {
            let target_dir = default_data_dir();
            let legacy_dir = find_legacy_data_dir();
            let imported_legacy_data = legacy_dir.exists() && legacy_dir != target_dir;
            if imported_legacy_data {
                migrate_data_files(&legacy_dir, &target_dir, false)?;
            }
            std::fs::create_dir_all(&target_dir)
                .map_err(|error| format!("无法创建便携版数据目录: {}", error))?;
            write_data_dir_config(&target_dir)?;
            return Ok(StorageInitializationResult {
                data_dir: target_dir.to_string_lossy().to_string(),
                imported_legacy_data,
                warning: None,
            });
        }
    }

    let data_dir = ensure_data_dir();
    Ok(StorageInitializationResult {
        data_dir: data_dir.to_string_lossy().to_string(),
        imported_legacy_data: false,
        warning: None,
    })
}

/// 修改数据目录
///
/// 顺序：复制受管数据 → 切换指针 → 删除旧目录中的受管数据。
/// 删除放在最后且只针对本程序命名的条目：切换成功前不动旧数据，切换后旧副本
/// 也不该继续被当成有效数据（用户可能手工回填到旧目录）。
#[tauri::command]
pub fn set_data_dir(new_path: String) -> Result<DataDirSwitchResult, String> {
    let old_dir = get_data_dir();
    let new_dir = PathBuf::from(&new_path);

    if new_path.trim().is_empty() {
        return Err("数据目录路径不能为空".to_string());
    }

    if comparable_path(&old_dir) == comparable_path(&new_dir) {
        return Ok(DataDirSwitchResult {
            data_dir: new_path,
            copied_files: 0,
            removed_entries: 0,
            warning: None,
        });
    }

    let copied_files = if old_dir.exists() {
        migrate_data_files(&old_dir, &new_dir, true)?
    } else {
        std::fs::create_dir_all(&new_dir)
            .map_err(|e| format!("无法创建数据目录: {}", e))?;
        0
    };

    // 数据复制完成后再切换指针，避免中断时指向半完成目录。
    write_data_dir_config(&new_dir)?;

    // 旧数据清理失败不影响本次切换，只作为提示返回给用户
    let (removed_entries, warning) = match remove_managed_data_entries(&old_dir) {
        Ok(removed) => (removed, None),
        Err(error) => (
            0,
            Some(format!(
                "数据已切换到新目录，但旧目录中的残留数据未能删除：{}",
                error
            )),
        ),
    };

    Ok(DataDirSwitchResult {
        data_dir: new_path,
        copied_files,
        removed_entries,
        warning,
    })
}

// ============================================================================
// 自定义文件夹持久化
// ============================================================================

/// 读取自定义文件夹列表
pub fn load_custom_folders(path: &Path) -> Vec<CustomFolderEntry> {
    if !path.exists() { return Vec::new(); }
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str::<Vec<CustomFolderEntry>>(&s).ok())
        .unwrap_or_default()
}

/// 保存自定义文件夹列表
pub fn save_custom_folders(path: &Path, folders: &[CustomFolderEntry]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("创建配置目录失败: {}", e))?;
    }
    let json = serde_json::to_string_pretty(folders)
        .map_err(|e| format!("序列化失败: {}", e))?;
    std::fs::write(path, &json)
        .map_err(|e| format!("写入失败: {}", e))?;
    // 数据目录被误删时自定义文件夹列表同样会丢，落盘后同步镜像一份
    crate::storage::mirror::mirror_custom_folders(folders);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recursive_copy_preserves_managed_files_without_copying_temp_files() {
        let suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("系统时间应有效")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("viap-data-dir-test-{suffix}"));
        let source = root.join("source");
        let target = root.join("target");
        std::fs::create_dir_all(source.join("cache/icons")).expect("创建测试源目录失败");
        std::fs::write(source.join("migration_history.json"), "history")
            .expect("写入历史测试文件失败");
        std::fs::write(source.join("cache/icons/icon.png"), "icon")
            .expect("写入图标测试文件失败");
        std::fs::write(source.join("ui_settings.json.tmp"), "incomplete")
            .expect("写入临时测试文件失败");
        // 数据目录可能同时是迁移目标根目录，里面的应用数据绝不能被一起搬走
        std::fs::create_dir_all(source.join("Wand")).expect("创建应用数据目录失败");
        std::fs::write(source.join("Wand/payload.bin"), "app-data")
            .expect("写入应用数据失败");
        std::fs::create_dir_all(&target).expect("创建测试目标目录失败");
        std::fs::write(target.join("migration_history.json"), "existing")
            .expect("写入目标测试文件失败");

        migrate_data_files(&source, &target, false).expect("递归复制测试失败");

        assert_eq!(
            std::fs::read_to_string(target.join("migration_history.json")).unwrap(),
            "existing"
        );
        assert_eq!(
            std::fs::read_to_string(target.join("cache/icons/icon.png")).unwrap(),
            "icon"
        );
        assert!(!target.join("ui_settings.json.tmp").exists());
        assert!(!target.join("Wand").exists(), "非受管目录不应被复制");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn cleanup_removes_only_managed_entries() {
        let suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("系统时间应有效")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("viap-data-dir-cleanup-{suffix}"));
        std::fs::create_dir_all(root.join("cache/icons")).expect("创建缓存目录失败");
        std::fs::create_dir_all(root.join("Wand")).expect("创建应用数据目录失败");
        // 与 Viap 缓存同名的应用目录必须保持原样
        std::fs::create_dir_all(root.join("cache")).expect("创建同名缓存目录失败");
        std::fs::write(root.join("migration_history.json"), "history").expect("写入历史失败");
        std::fs::write(root.join("cache/icons/icon.png"), "icon").expect("写入缓存失败");
        std::fs::write(root.join("Wand/payload.bin"), "app-data").expect("写入应用数据失败");

        let removed = remove_managed_data_entries(&root).expect("清理受管数据失败");

        assert_eq!(removed, 2);
        assert!(!root.join("migration_history.json").exists());
        assert!(!root.join("cache").exists());
        assert!(root.join("Wand/payload.bin").exists(), "应用数据必须保留");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn unrelated_cache_directory_is_not_treated_as_managed() {
        let suffix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("系统时间应有效")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("viap-data-dir-cache-{suffix}"));
        let target = root.join("target");
        // 应用自己的 cache 目录（没有本程序缓存文件）不能被视为受管目录
        std::fs::create_dir_all(root.join("cache/thumbnails")).expect("创建应用缓存目录失败");
        std::fs::write(root.join("cache/thumbnails/a.bin"), "app-cache").expect("写入应用缓存失败");

        let copied = migrate_data_files(&root, &target, true).expect("复制测试失败");
        assert_eq!(copied, 0);
        assert!(!target.join("cache").exists());

        let removed = remove_managed_data_entries(&root).expect("清理测试失败");
        assert_eq!(removed, 0);
        assert!(root.join("cache/thumbnails/a.bin").exists());
        let _ = std::fs::remove_dir_all(root);
    }
}
