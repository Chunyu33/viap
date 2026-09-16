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

/// 链接识别任务状态（Tauri 托管状态）
///
/// 与迁移/恢复共用取消标志会互相干扰（识别扫描期间取消会误中断迁移），
/// 因此单独持有一个取消标志。
pub struct LinkRecoveryState {
    /// 取消标志：前端调用 cancel_link_recovery 时设置为 true
    pub cancel_flag: Arc<AtomicBool>,
}

impl Default for LinkRecoveryState {
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
    /// 用于区分刷新前后的异步扫描，避免旧任务污染新任务状态。
    pub scan_id: Option<String>,
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
    /// 目标目录存在但为空，且原路径已不存在时表示没有可恢复数据。
    pub target_empty: bool,
    pub original_exists: bool,
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

// ============================================================================
// 迁移记录重建（原路径链接识别）
// ============================================================================

/// 链接识别结果条目
///
/// 由原路径侧扫描到的目录联接反推得到，字段全部可从前端展示与人工确认；
/// 真伪由 confidence + warnings 表达，绝不静默写入历史。
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RecoveredLinkEntry {
    /// 应用/文件夹名称（默认取原目录名）
    pub app_name: String,
    /// 原路径（现为目录联接）
    pub original_path: String,
    /// 联接指向的目标路径
    pub target_path: String,
    /// 推测的记录类型，前端可修改
    pub record_type: MigrationRecordType,
    /// 联接创建时间（近似迁移时间，Unix 毫秒；读取失败为 0）
    pub migrated_at: u64,
    /// 目标目录大小（字节）；未勾选统计时为 0
    pub size: u64,
    /// 置信度：high（可直接导入）/ medium（需确认）/ low（默认不勾选）
    pub confidence: String,
    /// 目标目录是否存在
    pub target_exists: bool,
    /// 目标目录存在但为空，说明没有可恢复数据
    pub target_empty: bool,
    /// 目标与原路径不在同一盘符
    pub cross_drive: bool,
    /// 现有历史中已存在同原路径的活跃记录
    pub already_recorded: bool,
    /// 判定依据与风险提示（前端直接展示，避免前端重复实现规则文案）
    pub warnings: Vec<String>,
}

/// 链接识别扫描结果
#[derive(Debug, Serialize, Deserialize)]
pub struct LinkRecoveryScanResult {
    pub entries: Vec<RecoveredLinkEntry>,
    /// 实际遍历的目录数
    pub scanned_dirs: u32,
    /// 因权限/系统目录被跳过的目录数
    pub skipped_dirs: u32,
    /// 达到扫描上限被提前截断
    pub truncated: bool,
    pub elapsed_ms: u64,
}

/// 导入项（前端回传，后端必须逐条重新校验，不信任前端结果）
#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct RecoveredLinkImport {
    pub app_name: String,
    pub original_path: String,
    pub target_path: String,
    pub record_type: MigrationRecordType,
    /// 联接创建时间，用于还原迁移时间
    pub migrated_at: u64,
    /// 目标大小（0 表示未统计）
    pub size: u64,
    /// 是否同时登记为自定义文件夹（仅对大文件夹类型有意义）
    #[serde(default)]
    pub register_custom_folder: bool,
}

/// 链接识别导入结果
#[derive(Debug, Serialize, Deserialize, Default)]
pub struct LinkRecoveryImportResult {
    /// 新增的迁移记录数
    pub imported: u32,
    /// 因重复或校验失败被跳过的条目数
    pub skipped: u32,
    /// 新登记的自定义文件夹数
    pub custom_folders_added: u32,
    /// 跳过原因（含路径，便于用户定位）
    pub failed: Vec<String>,
}

// ============================================================================
// 迁移数据镜像备份
// ============================================================================

/// 镜像备份信息（供恢复弹窗展示是否可一键导入）
#[derive(Debug, Serialize, Deserialize)]
pub struct MirrorBackupInfo {
    /// 镜像文件是否存在
    pub exists: bool,
    /// 镜像目录路径
    pub path: String,
    /// 镜像中的迁移记录数（含非 active 状态）
    pub history_count: u32,
    pub custom_folder_count: u32,
    pub migrated_app_count: u32,
    /// 镜像写入时间（Unix 毫秒）
    pub saved_at: u64,
}

/// 镜像备份导入结果
#[derive(Debug, Serialize, Deserialize, Default)]
pub struct MirrorImportResult {
    pub history_added: u32,
    pub history_skipped: u32,
    pub custom_folders_added: u32,
    pub migrated_apps_added: u32,
}
