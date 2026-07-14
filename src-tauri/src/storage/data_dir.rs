// 数据目录管理模块
//
// 架构说明：
// - 安装版指针文件位于 %APPDATA%/viap.json，便携版位于程序同级 viap.json
// - 安装版默认数据目录为 %APPDATA%/viap/，便携版为程序同级 data/
// - 用户可在设置中修改数据目录，数据文件自动迁移
// - 启动时检测数据目录是否存在，缺失则自动重建

use std::path::{Path, PathBuf};

use crate::models::{DataDirConfig, CustomFolderEntry};

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
        if let Ok(contents) = std::fs::read_to_string(&config_path) {
            if let Ok(config) = serde_json::from_str::<DataDirConfig>(&contents) {
                let path = PathBuf::from(&config.data_dir);
                if !path.to_string_lossy().is_empty() {
                    return path;
                }
            }
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
    Ok(DataDirConfig { data_dir: dir.to_string_lossy().to_string() })
}

/// 迁移数据文件从旧目录到新目录
fn migrate_data_files(old_dir: &Path, new_dir: &Path) -> Result<(), String> {
    std::fs::create_dir_all(new_dir)
        .map_err(|e| format!("无法创建数据目录: {}", e))?;

    let data_files = [
        "migration_history.json",
        "migration_history.json.bak",
        "custom_folders.json",
    ];

    for filename in &data_files {
        let old_path = old_dir.join(filename);
        let new_path = new_dir.join(filename);
        if old_path.exists() {
            std::fs::copy(&old_path, &new_path)
                .map_err(|e| format!("迁移文件失败 {}: {}", filename, e))?;
        }
    }
    Ok(())
}

/// 修改数据目录
/// 将数据文件从旧目录迁移到新目录，原子写入指针文件
#[tauri::command]
pub fn set_data_dir(new_path: String) -> Result<String, String> {
    let old_dir = get_data_dir();
    let new_dir = PathBuf::from(&new_path);

    if old_dir == new_dir {
        return Ok(new_path);
    }

    if new_path.trim().is_empty() {
        return Err("数据目录路径不能为空".to_string());
    }

    std::fs::create_dir_all(&new_dir)
        .map_err(|e| format!("无法创建数据目录: {}", e))?;

    if old_dir.exists() {
        migrate_data_files(&old_dir, &new_dir)?;
    }

    // 原子写入：先写临时文件再重命名，防止写入中断电损坏配置
    let config = DataDirConfig { data_dir: new_path.clone() };
    let json = serde_json::to_string_pretty(&config)
        .map_err(|e| format!("序列化配置失败: {}", e))?;
    let config_path = get_config_path();
    let temp_config = config_path.with_extension("json.tmp");
    std::fs::write(&temp_config, &json)
        .map_err(|e| format!("写入配置文件失败: {}", e))?;
    std::fs::rename(&temp_config, &config_path)
        .map_err(|e| format!("配置文件重命名失败: {}", e))?;

    Ok(new_path)
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
    Ok(())
}
