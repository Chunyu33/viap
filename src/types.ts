// Viap 类型定义文件

/**
 * 已安装应用信息接口
 * 对应 Rust 后端的 InstalledApp 结构体
 */
export interface InstalledApp {
  // 应用显示名称
  display_name: string;
  // 安装位置路径
  install_location: string;
  // 应用图标路径
  display_icon: string;
  // 预估大小（KB）
  estimated_size: number;
  // 应用图标的 Base64 编码数据（PNG 格式）
  // 如果提取失败则为空字符串
  // @deprecated 迁移至 icon_url 自定义协议
  icon_base64: string;
  // 图标自定义协议 URL（如 "orbit://icon.C:/Program Files/App/app.exe"）
  icon_url: string;
  // 应用对应注册表路径（用于强力卸载）
  registry_path: string;
  // 发布商（用于强力卸载残留匹配）
  publisher: string;
}

/**
 * 强力卸载残留项
 * 对应 Rust 后端的 LeftoverItem 结构体
 */
export interface LeftoverItem {
  // 路径（文件、目录或注册表路径）
  path: string;
  // 项目类型：Folder / File / Registry
  item_type: string;
  // 大小（MB）
  size_mb: number;
  // 是否默认选中
  selected: boolean;
}

/**
 * 卸载结果接口
 * 对应 Rust 后端的 UninstallResult 结构体
 */
export interface UninstallResult {
  // 是否成功执行卸载流程
  success: boolean;
  // 安装目录是否已完全移除；部分选择删除时仍可能为 false
  application_removed: boolean;
  // 返回消息
  message: string;
  // 实际执行的卸载命令
  command: string | null;
  // 扫描出的残留项目
  leftovers: LeftoverItem[];
}

/**
 * 卸载命令预览接口
 * 对应 Rust 后端的 UninstallPreview 结构体
 */
export interface UninstallPreview {
  commands: string[];
}

/**
 * 清理结果接口
 * 对应 Rust 后端的 CleanupResult 结构体
 */
export interface CleanupResult {
  // 是否全部成功
  success: boolean;
  // 返回消息
  message: string;
  // 成功清理数量
  cleaned_count: number;
  // 清理失败项
  failed_items: string[];
}

/** 幽灵链接预览条目 */
export interface GhostLinkEntry {
  record_id: string;
  app_name: string;
  original_path: string;
  target_path: string;
  size: number;
  /** 损坏类型：target_missing | junction_broken | original_missing */
  damage_type: 'target_missing' | 'junction_broken' | 'original_missing';
}

/** 幽灵链接预览结果 */
export interface GhostLinkPreview {
  entries: GhostLinkEntry[];
  total_size: number;
}

/**
 * 磁盘使用信息接口
 * 对应 Rust 后端的 DiskUsage 结构体
 */
export interface DiskUsage {
  // 磁盘盘符（如 "C:", "D:"）
  mount_point: string;
  // 磁盘名称（如 "系统", "数据"）
  name: string;
  // 总容量（字节）
  total_space: number;
  // 可用空间（字节）
  free_space: number;
  // 已使用空间（字节）
  used_space: number;
  // 使用百分比
  usage_percent: number;
  // 是否为系统盘
  is_system: boolean;
}

/**
 * Tab 页面类型枚举
 */
export type TabType = 'migration' | 'folders' | 'history' | 'settings';

/**
 * 大文件夹类型枚举
 */
export type LargeFolderType = 'System' | 'AppData' | 'Custom';

/**
 * 大文件夹信息接口
 * 对应 Rust 后端的 LargeFolder 结构体
 */
export interface LargeFolder {
  // 文件夹唯一标识
  id: string;
  // 显示名称
  display_name: string;
  // 文件夹完整路径
  path: string;
  // 文件夹大小（字节）
  size: number;
  // 文件夹类型
  folder_type: LargeFolderType;
  // 是否已经是 Junction（已迁移）
  is_junction: boolean;
  // Junction 目标路径
  junction_target: string | null;
  // 关联的应用进程名称
  app_process_names: string[];
  // 图标标识
  icon_id: string;
  // 是否存在
  exists: boolean;
}

/**
 * 大文件夹大小更新事件
 * 后台异步计算大小后通过 "large-folder-size" 事件推送
 */
export interface LargeFolderSizeEvent {
  folder_id: string;
  size: number;
  // 用于忽略已经过期的应用数据扫描完成事件。
  scan_id?: string | null;
}

/**
 * 数据目录配置
 * 对应 Rust 后端的 DataDirConfig 结构体
 */
export interface DataDirConfig {
  data_dir: string;
  // 便携版默认 data 目录标记，前端仅用于兼容扩展字段，不参与路径计算。
  portable_default?: boolean;
}

/**
 * 应用数据模板条目
 * 对应 Rust 后端的 AppDataTemplate 结构体
 */
export interface AppDataTemplate {
  id: string;
  display_name: string;
  icon_id: string;
  process_names: string[];
  path: string | null;
}

/**
 * 迁移结果接口
 * 对应 Rust 后端的 MigrationResult 结构体
 */
export interface MigrationResult {
  // 是否成功
  success: boolean;
  // 结果消息
  message: string;
  // 新的安装路径（成功时返回）
  new_path: string | null;
}

/**
 * 进程锁检测结果接口
 * 对应 Rust 后端的 ProcessLockResult 结构体
 */
export interface ProcessLockResult {
  // 是否有进程占用
  is_locked: boolean;
  // 占用进程名称列表
  processes: string[];
}

/**
 * 迁移步骤枚举
 */
export type MigrationStep =
  | 'idle'           // 空闲状态
  | 'checking'       // 检查进程锁
  | 'counting'       // 扫描文件
  | 'copying'        // 复制文件
  | 'verifying'      // 校验完整性
  | 'linking'        // 创建链接
  | 'success'        // 迁移成功
  | 'error';         // 迁移失败

/**
 * 迁移进度事件接口
 * 对应 Rust 后端的 MigrationProgressEvent 结构体
 * 通过 Tauri Event "migration-progress" 推送
 */
export interface MigrationProgressEvent {
  task_id: string;
  percent: number;
  step: string;
  message: string;
  copied_size: number;
  total_size: number;
}

/**
 * 迁移记录类型枚举
 */
export type MigrationRecordType = 'App' | 'LargeFolder';

/**
 * 迁移历史记录接口
 * 对应 Rust 后端的 MigrationRecord 结构体
 */
export interface MigrationRecord {
  // 唯一标识符
  id: string;
  // 应用/文件夹名称
  app_name: string;
  // 原始路径
  original_path: string;
  // 目标路径
  target_path: string;
  // 迁移大小（字节）
  size: number;
  // 迁移时间（Unix 时间戳，毫秒）
  migrated_at: number;
  // 状态
  status: string;
  // 记录类型：App（应用）或 LargeFolder（大文件夹）
  record_type: MigrationRecordType;
}
