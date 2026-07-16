// Viap 数据模型定义
// 集中管理所有前后端共享的数据结构体、枚举和序列化类型

use serde::{Deserialize, Serialize};
use std::sync::{Arc, atomic::AtomicBool};

// ============================================================================
// 迁移状态管理
// ============================================================================

/// 迁移任务状态（Tauri 托管状态）
/// 用于在前后端之间传递取消信号
pub struct MigrationState {
    /// 取消标志：前端调用 cancel_migration 时设置为 true
    pub cancel_flag: Arc<AtomicBool>,
}

impl Default for MigrationState {
    fn default() -> Self {
        Self { cancel_flag: Arc::new(AtomicBool::new(false)) }
    }
}

// ============================================================================
// 应用与磁盘信息
// ============================================================================

/// 已安装应用信息结构体
/// 包含从 Windows 注册表读取的应用基本信息
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct InstalledApp {
    /// 应用显示名称
    pub display_name: String,
    /// 安装位置路径
    pub install_location: String,
    /// 应用图标路径
    pub display_icon: String,
    /// 预估大小（KB）
    pub estimated_size: u64,
    /// 应用图标的 Base64 编码数据（PNG 格式），提取失败则为空字符串
    /// @deprecated 迁移至 icon_url 自定义协议，保留以兼容前端旧版本
    pub icon_base64: String,
    /// 图标自定义协议 URL（如 "orbit://icon.C:/Program Files/App/app.exe"）
    /// 前端优先使用此字段渲染图标，回退到 icon_base64
    pub icon_url: String,
    /// 应用对应注册表路径（用于后续卸载）
    pub registry_path: String,
    /// 发布商（用于强力卸载残留匹配）
    pub publisher: String,
}

/// 磁盘使用信息结构体
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct DiskUsage {
    /// 磁盘盘符（如 "C:", "D:"）
    pub mount_point: String,
    /// 磁盘名称（如 "系统", "数据"）
    pub name: String,
    /// 总容量（字节）
    pub total_space: u64,
    /// 可用空间（字节）
    pub free_space: u64,
    /// 已使用空间（字节）
    pub used_space: u64,
    /// 使用百分比
    pub usage_percent: f64,
    /// 是否为系统盘
    pub is_system: bool,
}

// ============================================================================
// 大文件夹相关
// ============================================================================

/// 大文件夹类型枚举
/// 区分系统文件夹和应用数据文件夹，用于前端显示不同的风险提示
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub enum LargeFolderType {
    /// 系统文件夹（桌面、文档、下载等）— 迁移风险较高
    System,
    /// 应用数据文件夹（微信、钉钉等）— 迁移风险较低
    AppData,
    /// 自定义文件夹（用户手动添加）
    Custom,
}

/// 大文件夹信息结构体
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct LargeFolder {
    pub id: String,
    pub display_name: String,
    pub path: String,
    /// 文件夹大小（字节），由后台异步计算
    pub size: u64,
    pub folder_type: LargeFolderType,
    /// 是否已经是 Junction（已迁移）
    pub is_junction: bool,
    /// Junction 目标路径（如果已迁移）
    pub junction_target: Option<String>,
    /// 关联的应用进程名（用于迁移前进程检测）
    pub app_process_names: Vec<String>,
    /// 图标标识（前端 iconMap 的 key）
    pub icon_id: String,
    /// 文件夹是否存在
    pub exists: bool,
}

/// 大文件夹大小更新事件（后台异步计算后推送给前端）
#[derive(Debug, Clone, Serialize)]
pub struct LargeFolderSizeEvent {
    pub folder_id: String,
    pub size: u64,
}

// ============================================================================
// 数据目录管理
// ============================================================================

/// 数据目录配置（存储在指针文件 %APPDATA%/viap.json 中）
#[derive(Debug, Serialize, Deserialize)]
pub struct DataDirConfig {
    pub data_dir: String,
    /// 便携版默认 data 目录标记，用于程序目录整体移动后的路径修复。
    #[serde(default, skip_serializing_if = "is_false")]
    pub portable_default: bool,
}

fn is_false(value: &bool) -> bool {
    !*value
}

/// 自定义文件夹持久化条目
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CustomFolderEntry {
    pub id: String,
    pub path: String,
    pub display_name: String,
}

// ============================================================================
// 应用数据模板
// ============================================================================

/// 应用数据模板条目
/// 定义哪些应用的数据目录需要监控
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppDataTemplate {
    /// 唯一标识（内置类型对应 detector 模块，如 "wechat", "qq"）
    pub id: String,
    /// 显示名称
    pub display_name: String,
    /// 图标标识（前端 iconMap 的 key）
    #[serde(default = "default_icon_id")]
    pub icon_id: String,
    /// 关联进程名（用于迁移前进程检测）
    #[serde(default = "default_process_names")]
    pub process_names: Vec<String>,
    /// 可选的固定路径（支持 %VAR% 环境变量展开）
    #[serde(default)]
    pub path: Option<String>,
}

fn default_icon_id() -> String { "folder".to_string() }
fn default_process_names() -> Vec<String> { vec![] }

// ============================================================================
// 迁移核心类型
// ============================================================================

/// 迁移结果结构体
#[derive(Debug, Serialize, Deserialize)]
pub struct MigrationResult {
    pub success: bool,
    pub message: String,
    /// 新的安装路径（成功时返回）
    pub new_path: Option<String>,
}

/// 进程锁检测结果
#[derive(Debug, Serialize, Deserialize)]
pub struct ProcessLockResult {
    pub is_locked: bool,
    /// 占用进程名称列表
    pub processes: Vec<String>,
}

// ============================================================================
// 迁移历史记录
// ============================================================================

/// 迁移记录类型枚举
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub enum MigrationRecordType {
    App,
    LargeFolder,
}

/// 迁移历史记录结构体
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct MigrationRecord {
    /// 唯一标识符（格式: mig_<timestamp>）
    pub id: String,
    /// 应用/文件夹名称
    pub app_name: String,
    /// 原始路径（迁移前的位置）
    pub original_path: String,
    /// 目标路径（迁移后的实际存储位置）
    pub target_path: String,
    /// 迁移大小（字节）
    pub size: u64,
    /// 迁移时间（Unix 时间戳，毫秒）
    pub migrated_at: u64,
    /// 状态：active（已迁移）、restored（已恢复）、ghost_cleaned（已清理）
    pub status: String,
    /// 记录类型，旧记录默认为 App（向后兼容）
    #[serde(default = "default_record_type")]
    pub record_type: MigrationRecordType,
}

fn default_record_type() -> MigrationRecordType { MigrationRecordType::App }

/// 历史记录持久化存储结构
#[derive(Debug, Serialize, Deserialize)]
pub struct HistoryStorage {
    pub version: u32,
    pub records: Vec<MigrationRecord>,
}

/// 链接健康状态检查结果
#[derive(Debug, Serialize, Deserialize)]
pub struct LinkStatusResult {
    pub healthy: bool,
    pub target_exists: bool,
    pub is_junction: bool,
    pub error: Option<String>,
}

/// 幽灵链接预览条目
#[derive(Debug, Serialize, Deserialize)]
pub struct GhostLinkEntry {
    pub record_id: String,
    pub app_name: String,
    pub original_path: String,
    pub target_path: String,
    pub size: u64,
    /// 损坏类型：target_missing | junction_broken | original_missing
    pub damage_type: String,
}

/// 幽灵链接预览结果
#[derive(Debug, Serialize, Deserialize)]
pub struct GhostLinkPreview {
    pub entries: Vec<GhostLinkEntry>,
    pub total_size: u64,
}

/// 清理结果结构体
#[derive(Debug, Serialize, Deserialize)]
pub struct CleanupResult {
    pub cleaned_count: u32,
    pub cleaned_size: u64,
    pub errors: Vec<String>,
}

/// 迁移统计信息
#[derive(Debug, Serialize, Deserialize)]
pub struct MigrationStats {
    /// 总共节省的空间（字节）
    pub total_space_saved: u64,
    /// 当前活跃的迁移数量
    pub active_migrations: u32,
    /// 已恢复的迁移数量
    pub restored_count: u32,
    /// 应用迁移数量
    pub app_migrations: u32,
    /// 文件夹迁移数量
    pub folder_migrations: u32,
}
