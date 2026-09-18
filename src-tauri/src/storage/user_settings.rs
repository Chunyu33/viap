// 用户界面设置持久化模块
//
// 设置文件放在 Viap 数据目录内，便携版复制整个目录后可以继续保留主题、字号和迁移偏好。

use std::path::PathBuf;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use super::data_dir::ensure_data_dir;

const SETTINGS_FILE_NAME: &str = "ui_settings.json";

lazy_static::lazy_static! {
    // 多次快速切换设置时串行写入，避免多个调用同时操作同一个临时文件。
    static ref SETTINGS_WRITE_LOCK: Mutex<()> = Mutex::new(());
}

fn default_theme() -> String {
    "system".to_string()
}

fn default_font_size() -> u8 {
    13
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserSettings {
    #[serde(default)]
    pub default_app_target_path: String,
    #[serde(default)]
    pub default_data_target_path: String,
    #[serde(default = "default_true")]
    pub use_recycle_bin: bool,
    #[serde(default)]
    pub show_scan_debug: bool,
    #[serde(default = "default_font_size")]
    pub font_size_px: u8,
    #[serde(default = "default_theme")]
    pub theme: String,
    /// 自动备份迁移数据（历史、自定义文件夹、应用兜底数据）；关闭后只保留手动备份
    #[serde(default = "default_true")]
    pub auto_backup_enabled: bool,
    /// 跳过迁移前的逐文件占用检测（大目录迁移更快，但占用问题会在复制阶段才暴露）
    #[serde(default)]
    pub skip_lock_check: bool,
    /// 窗口宽度（逻辑像素，随窗口拖动单独保存）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_width: Option<u32>,
    /// 窗口高度（逻辑像素，随窗口拖动单独保存）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window_height: Option<u32>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserSettingsLoadResult {
    pub settings: UserSettings,
    pub initialized: bool,
}

fn default_true() -> bool {
    true
}

/// 字号可选范围：上限放宽以便 4K 屏幕在 100% 缩放下也能看清
pub(crate) const MIN_FONT_SIZE_PX: u8 = 12;
pub(crate) const MAX_FONT_SIZE_PX: u8 = 20;

/// 窗口尺寸合法范围（与 tauri.conf.json 的 minWidth/minHeight 保持一致）
const MIN_WINDOW_WIDTH: u32 = 800;
const MIN_WINDOW_HEIGHT: u32 = 540;
/// 上限只用于拦截异常值（如显示器信息异常），不是实际的屏幕限制
const MAX_WINDOW_SIDE: u32 = 20_000;

impl Default for UserSettings {
    fn default() -> Self {
        Self {
            default_app_target_path: String::new(),
            default_data_target_path: String::new(),
            use_recycle_bin: true,
            show_scan_debug: false,
            font_size_px: default_font_size(),
            theme: default_theme(),
            auto_backup_enabled: true,
            skip_lock_check: false,
            window_width: None,
            window_height: None,
        }
    }
}

impl UserSettings {
    fn normalized(mut self) -> Self {
        // 后端再次校验边界，避免旧配置或外部修改把前端控件带到异常状态。
        self.font_size_px = self.font_size_px.clamp(MIN_FONT_SIZE_PX, MAX_FONT_SIZE_PX);
        if !matches!(self.theme.as_str(), "light" | "dark" | "system") {
            self.theme = default_theme();
        }
        self
    }
}

/// 界面设置文件路径（设置页「数据管理」也据此展示位置）
pub(crate) fn settings_path() -> PathBuf {
    ensure_data_dir().join(SETTINGS_FILE_NAME)
}

/// 读取当前用户设置（文件缺失或损坏时回退默认值）
///
/// 供后端内部判断自动备份开关等行为，读失败不能中断调用方流程。
pub(crate) fn load_current_settings() -> UserSettings {
    let path = settings_path();
    if !path.exists() {
        return UserSettings::default();
    }
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|json| serde_json::from_str::<UserSettings>(&json).ok())
        .map(UserSettings::normalized)
        .unwrap_or_default()
}

/// 读取设置文件；文件不存在时返回默认值并交给前端导入旧 localStorage。
#[tauri::command]
pub fn get_user_settings() -> Result<UserSettingsLoadResult, String> {
    let path = settings_path();
    if !path.exists() {
        return Ok(UserSettingsLoadResult {
            settings: UserSettings::default(),
            initialized: false,
        });
    }

    let json =
        std::fs::read_to_string(&path).map_err(|error| format!("读取用户设置失败: {}", error))?;
    let settings = serde_json::from_str::<UserSettings>(&json)
        .map_err(|error| format!("解析用户设置失败: {}", error))?
        .normalized();
    Ok(UserSettingsLoadResult {
        settings,
        initialized: true,
    })
}

/// 保存完整用户设置，采用临时文件替换避免断电留下半个 JSON。
///
/// 窗口尺寸由 save_window_state 单独维护，前端提交设置时可能不带这两个字段，
/// 这里从磁盘上的旧值补齐，避免保存其它设置时把记忆的窗口尺寸清掉。
#[tauri::command]
pub fn save_user_settings(settings: UserSettings) -> Result<(), String> {
    let mut merged = settings.normalized();
    let previous = load_current_settings();
    if merged.window_width.is_none() {
        merged.window_width = previous.window_width;
    }
    if merged.window_height.is_none() {
        merged.window_height = previous.window_height;
    }
    write_settings(&merged)
}

/// 记录窗口尺寸（前端在拖动结束后防抖调用）
///
/// 尺寸必须是逻辑像素且在合法范围内；越界值直接忽略，避免写入异常值导致
/// 下次启动窗口不可用。
#[tauri::command]
pub fn save_window_state(width: u32, height: u32) -> Result<(), String> {
    if !(MIN_WINDOW_WIDTH..=MAX_WINDOW_SIDE).contains(&width)
        || !(MIN_WINDOW_HEIGHT..=MAX_WINDOW_SIDE).contains(&height)
    {
        return Ok(());
    }

    let mut settings = load_current_settings();
    if settings.window_width == Some(width) && settings.window_height == Some(height) {
        // 尺寸没变就不落盘，避免无意义的磁盘写入
        return Ok(());
    }
    settings.window_width = Some(width);
    settings.window_height = Some(height);
    write_settings(&settings)
}

/// 读取记忆的窗口尺寸（启动时在窗口显示前应用）
pub(crate) fn saved_window_size() -> Option<(u32, u32)> {
    let settings = load_current_settings();
    match (settings.window_width, settings.window_height) {
        (Some(width), Some(height)) => Some((width, height)),
        _ => None,
    }
}

/// 原子写入设置文件；写入期间串行化，避免多个调用同时操作同一个临时文件
fn write_settings(settings: &UserSettings) -> Result<(), String> {
    let _guard = SETTINGS_WRITE_LOCK
        .lock()
        .map_err(|_| "用户设置写入锁已损坏，请重启应用后重试".to_string())?;
    let path = settings_path();
    let json = serde_json::to_string_pretty(settings)
        .map_err(|error| format!("序列化用户设置失败: {}", error))?;
    let temp_path = path.with_extension("json.tmp");
    std::fs::write(&temp_path, json).map_err(|error| format!("写入用户设置失败: {}", error))?;
    std::fs::rename(&temp_path, &path).map_err(|error| format!("更新用户设置失败: {}", error))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalized_settings_reject_invalid_theme_and_font_size() {
        let settings = UserSettings {
            theme: "unknown".to_string(),
            font_size_px: 99,
            ..UserSettings::default()
        }
        .normalized();

        assert_eq!(settings.theme, "system");
        assert_eq!(settings.font_size_px, MAX_FONT_SIZE_PX);
    }

    #[test]
    fn settings_use_camel_case_json_for_frontend_ipc() {
        let json = serde_json::to_string(&UserSettings::default()).unwrap();
        assert!(json.contains("defaultAppTargetPath"));
        assert!(json.contains("fontSizePx"));
        assert!(!json.contains("default_app_target_path"));
    }

    #[test]
    fn window_size_fields_are_optional_and_round_trip() {
        // 未设置过窗口尺寸时不应写出这两个字段，避免旧版本读到无意义的值
        let default_json = serde_json::to_string(&UserSettings::default()).unwrap();
        assert!(!default_json.contains("windowWidth"));

        let settings = UserSettings {
            window_width: Some(1280),
            window_height: Some(800),
            ..UserSettings::default()
        };
        let json = serde_json::to_string(&settings).unwrap();
        assert!(json.contains("\"windowWidth\":1280"));
        let restored: UserSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.window_width, Some(1280));
        assert_eq!(restored.window_height, Some(800));
    }
}
