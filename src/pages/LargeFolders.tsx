// 数据迁移页面 — 桌面工具风格
// 紧凑行布局，弱化操作视觉

import { useEffect, useState, useMemo, useCallback, useContext, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { open, confirm } from '@tauri-apps/plugin-dialog';
import {
  RefreshCw, FolderOpen, AlertTriangle,
  Link2, Undo2, Plus, X, Loader2, Check,
  Monitor, FileText, Download, Image, Video,
  MessageCircle, Building2, Users, Phone, Bird, Globe, Code, Package, ArrowRightLeft,
} from 'lucide-react';
import Toast, { useToast } from '../components/Toast';
import EmptyState from '../components/EmptyState';
import MigrationModal from '../components/MigrationModal';
import TargetPickerDialog from '../components/TargetPickerDialog';
import { useDangerousPathCheck, WarningInfo } from '../hooks/useDangerousPathCheck';
import WarningConfirmDialog from '../components/WarningConfirmDialog';
import { useViapStore } from '../store';
import {
  LargeFolder, ProcessLockResult, LargeFolderSizeEvent,
  MigrationProgressEvent,
  MigrationResult, MigrationStep, TabType,
} from '../types';
import { TabNavigationContext } from '../App';

function formatSize(bytes: number): string {
  if (bytes === 0) return '--';
  const k = 1024;
  const sizes = ['B', 'KB', 'MB', 'GB', 'TB'];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return parseFloat((bytes / Math.pow(k, i)).toFixed(2)) + ' ' + sizes[i];
}

function getFolderIcon(iconId: string) {
  const map: Record<string, React.ReactNode> = {
    desktop: <Monitor className="w-4 h-4" />,
    documents: <FileText className="w-4 h-4" />,
    downloads: <Download className="w-4 h-4" />,
    pictures: <Image className="w-4 h-4" />,
    videos: <Video className="w-4 h-4" />,
    wechat: <MessageCircle className="w-4 h-4" />,
    wxwork: <Building2 className="w-4 h-4" />,
    qq: <Users className="w-4 h-4" />,
    dingtalk: <Phone className="w-4 h-4" />,
    feishu: <Bird className="w-4 h-4" />,
    chrome_cache: <Globe className="w-4 h-4" />,
    edge_cache: <Globe className="w-4 h-4" />,
    vscode_extensions: <Code className="w-4 h-4" />,
    npm_global: <Package className="w-4 h-4" />,
  };
  return map[iconId] || <FolderOpen className="w-4 h-4" />;
}

/** 从 localStorage 读取默认数据迁移目录，仅非 C 盘路径有效 */
function loadDataDefaultTarget(): string | null {
  try {
    const saved = JSON.parse(localStorage.getItem('viap_settings') || '{}');
    const path = saved.defaultDataTargetPath;
    if (path && typeof path === 'string' && path.length > 0) {
      // C 盘路径视为无效，需由用户重新选择
      if (path.startsWith('C:') || path.startsWith('c:')) return null;
      return path;
    }
  } catch { /* 设置读取失败时忽略 */ }
  return null;
}

/**
 * 解析数据迁移目标目录：优先使用默认设置，否则引导配置或手动选择
 * 返回选中的目标路径，null 表示用户取消操作
 *
 * 使用自定义 TargetPickerDialog 替代原生 confirm 弹窗，
 * 以区分「使用默认」「自定义目录」和「X 关闭」三个独立操作。
 */
async function resolveDataMigrationTarget(
  defaultPath: string | null,
  folderName: string,
  navigateToSettings: ((tab: TabType) => void) | null,
  showTargetPicker: (defaultPath: string, itemName: string) => Promise<'default' | 'custom' | null>,
): Promise<string | null> {
  if (defaultPath) {
    const action = await showTargetPicker(defaultPath, `文件夹 "${folderName}" 将迁移到此目录`);
    if (action === 'default') return defaultPath;
    if (action === null) return null;
    // action === 'custom' → 继续往下打开文件夹选择器
  } else {
    // 无有效默认路径，引导前往设置
    const goSettings = await confirm(
      '未设置默认迁移目录。\n\n是否前往设置页进行配置？',
      { title: '未配置迁移目录', kind: 'info', okLabel: '前往设置', cancelLabel: '取消' },
    );
    if (goSettings) {
      navigateToSettings?.('settings');
    }
    return null;
  }

  // 用户选择自定义目录
  const targetDir = await open({
    directory: true,
    multiple: false,
    title: `选择迁移目录文件夹 - ${folderName}`,
  });
  return targetDir as string | null;
}

function RiskConfirmModal({
  isOpen, folder, onConfirm, onCancel,
}: {
  isOpen: boolean; folder: LargeFolder | null; onConfirm: () => void; onCancel: () => void;
}) {
  if (!isOpen || !folder) return null;
  const isSystem = folder.folder_type === 'System';

  return (
    <div className="fixed inset-0 z-[1000] flex items-center justify-center" style={{ background: 'var(--bg-modal-overlay)' }}>
      <div className="animate-modal-in rounded-lg p-6 w-[440px]" style={{ background: 'var(--bg-modal)', border: '1px solid var(--border-color)', boxShadow: 'var(--shadow-lg)' }}>
        <div className="flex items-center gap-3 mb-4">
          <div className="w-9 h-9 rounded flex items-center justify-center" style={{ background: isSystem ? 'var(--color-danger-light)' : 'var(--color-warning-light)' }}>
            <AlertTriangle className="w-5 h-5" style={{ color: isSystem ? 'var(--color-danger)' : 'var(--color-warning)' }} />
          </div>
          <div>
            <h3 className="text-[14px] font-semibold" style={{ color: 'var(--text-primary)' }}>
              {isSystem ? '高风险操作' : '确认迁移'}
            </h3>
            <p className="text-[11px]" style={{ color: 'var(--text-tertiary)' }}>
              {folder.display_name} — {formatSize(folder.size)}
            </p>
          </div>
        </div>

        <div className="mb-5 text-[12px] leading-relaxed" style={{ color: 'var(--text-secondary)' }}>
          {isSystem ? (
            <>
              <p className="mb-2">迁移系统文件夹 <strong>{folder.display_name}</strong> 可能导致：</p>
              <ul className="list-disc pl-4 mb-2">
                <li>部分应用无法正常访问该文件夹</li>
                <li>Windows 系统功能异常</li>
                <li>OneDrive 等服务出现问题</li>
              </ul>
            </>
          ) : (
            <>
              <p className="mb-2">即将迁移 <strong>{folder.display_name}</strong>。</p>
              <ul className="list-disc pl-4">
                <li>请确认已关闭相关应用</li>
                <li>目标磁盘需有足够空间</li>
              </ul>
            </>
          )}
        </div>

        <div className="flex justify-end gap-2">
          <button onClick={onCancel} className="btn h-7 text-[12px]">取消</button>
          <button onClick={onConfirm} className="btn btn-danger h-7 text-[12px]">
            {isSystem ? '我了解风险，继续' : '确认迁移'}
          </button>
        </div>
      </div>
    </div>
  );
}

function FolderRow({
  folder, onMigrate, onRestore, onOpenFolder, onRemove,
  isMigrating, isRestoring,
  restoreProgress,
  showCheckbox, isSelected, onToggleSelect,
}: {
  folder: LargeFolder;
  onMigrate: (f: LargeFolder) => void;
  onRestore: (f: LargeFolder) => void;
  onOpenFolder: (path: string) => void;
  onRemove?: (f: LargeFolder) => void;
  isMigrating?: boolean;
  isRestoring?: boolean;
  restoreProgress?: number;
  showCheckbox?: boolean;
  isSelected?: boolean;
  onToggleSelect?: (f: LargeFolder) => void;
}) {
  const notFound = !folder.exists;
  const isSystem = folder.folder_type === 'System';
  const isCustom = folder.folder_type === 'Custom';

  const rowStyle: React.CSSProperties = {
    height: 'var(--row-height)' as unknown as string,
    padding: '0 8px',
    borderBottom: '1px solid var(--border-color)',
    opacity: notFound ? 0.4 : 1,
  } as React.CSSProperties;

  return (
    <div
      className="flex items-center gap-3"
      style={rowStyle}
      onMouseEnter={(e) => { (e.currentTarget as HTMLElement).style.background = 'var(--bg-row-hover)'; }}
      onMouseLeave={(e) => { (e.currentTarget as HTMLElement).style.background = 'transparent'; }}
    >
      {/* 批量选择复选框 — 已迁移文件夹不显示 */}
      {showCheckbox && !folder.is_junction && (
        <div
          className="flex items-center justify-center w-4 h-4 rounded border cursor-pointer flex-shrink-0"
          style={{
            borderColor: isSelected ? 'var(--color-primary)' : 'var(--border-color-strong)',
            background: isSelected ? 'var(--color-primary)' : 'transparent',
          }}
          onClick={(e) => { e.stopPropagation(); onToggleSelect?.(folder); }}
        >
          {isSelected && <Check className="w-3 h-3" style={{ color: '#fff' }} />}
        </div>
      )}
      {showCheckbox && folder.is_junction && (
        <div className="w-4 h-4 flex-shrink-0" />
      )}
      {/* icon */}
      <div className="w-7 h-7 rounded flex items-center justify-center flex-shrink-0" style={{ color: 'var(--text-secondary)' }}>
        {getFolderIcon(folder.icon_id)}
      </div>

      {/* info */}
      <div className="flex-1 min-w-0 flex items-center gap-3">
        <span className="text-[13px] font-medium truncate" style={{ color: 'var(--text-primary)', maxWidth: '200px' }}>
          {folder.display_name}
        </span>
        <div className="flex items-center gap-1.5 flex-shrink-0">
          {notFound && <span className="badge" style={{ color: 'var(--text-tertiary)' }}>未检测到</span>}
          {!notFound && isSystem && !folder.is_junction && (
            <span className="badge" style={{ color: 'var(--color-warning)', background: 'var(--color-warning-light)' }}>系统</span>
          )}
          {isCustom && <span className="badge badge-primary">自定义</span>}
          {folder.is_junction && <span className="badge badge-success"><Link2 className="w-2.5 h-2.5" />已迁移</span>}
        </div>
        <span className="text-[11px] truncate flex-1 min-w-0" style={{ color: 'var(--text-tertiary)' }} title={folder.path}>
          {folder.is_junction && folder.junction_target
            ? `→ ${folder.junction_target}`
            : notFound ? `默认: ${folder.path}` : folder.path}
        </span>
      </div>

      {/* size */}
      <span className="text-[11px] tabular-nums flex-shrink-0 w-20 text-right" style={{ color: 'var(--text-secondary)' }}>
        {notFound ? '--' : folder.is_junction ? '--' : formatSize(folder.size)}
      </span>

      {/* actions */}
      <div className="flex items-center gap-1 flex-shrink-0">
        {!notFound && (
          <button onClick={() => onOpenFolder(folder.path)} className="btn btn-ghost btn-icon" title="打开目录">
            <FolderOpen className="w-3.5 h-3.5" />
          </button>
        )}
        {isCustom && !folder.is_junction && onRemove && (
          <button onClick={() => onRemove(folder)} className="btn btn-ghost btn-icon" title="移除">
            <X className="w-3.5 h-3.5" />
          </button>
        )}

        {notFound ? (
          <span className="text-[11px] mr-2" style={{ color: 'var(--text-tertiary)' }}>不可用</span>
        ) : folder.is_junction ? (
          <button
            onClick={() => onRestore(folder)}
            disabled={isRestoring}
            className="btn btn-sm h-6 text-[11px]"
            style={isRestoring ? {
              // 行内按钮绘制恢复进度，避免另加列导致表格抖动。
              background: `linear-gradient(to right, var(--color-primary-light) 0%, var(--color-primary-light) ${Math.round(restoreProgress ?? 0)}%, transparent ${Math.round(restoreProgress ?? 0)}%)`,
              borderColor: 'var(--color-primary)',
              color: 'var(--color-primary)',
            } : undefined}
          >
            {isRestoring ? <Loader2 className="w-3 h-3 animate-spin" /> : <Undo2 className="w-3 h-3" />}
            {/* 恢复过程展示后端百分比，让大目录恢复时不再只有 loading。 */}
            {isRestoring ? `${Math.round(restoreProgress ?? 0)}%` : '恢复'}
          </button>
        ) : (
          <button onClick={() => onMigrate(folder)} disabled={isMigrating} className="btn btn-primary btn-sm h-6 text-[11px]">
            {isMigrating ? <Loader2 className="w-3 h-3 animate-spin" /> : null}
            {isMigrating ? '迁移中' : '迁移'}
          </button>
        )}
      </div>
    </div>
  );
}

/**
 * 前端危险路径检测（与后端 check_dangerous_path 保持同步）
 * 在 invoke 前给用户即时反馈，避免等待后端才报错
 * 返回错误消息字符串，或 null 表示安全
 */
export default function LargeFolders({ visible }: { visible: boolean }) {
  const storeApi = useViapStore;
  const [folders, setFolders] = useState<LargeFolder[]>(() => storeApi.getState().largeFolders);
  const [loading, setLoading] = useState(true);
  const [refreshing, setRefreshing] = useState(false);
  const [restoringFolderId, setRestoringFolderId] = useState<string | null>(null);
  const [restoreProgressMap, setRestoreProgressMap] = useState<Record<string, number>>({});

  // 迁移进度弹窗状态
  const [migrationModalOpen, setMigrationModalOpen] = useState(false);
  const [migrationStep, setMigrationStep] = useState<MigrationStep>('idle');
  const [migratingFolder, setMigratingFolder] = useState<LargeFolder | null>(null);
  const [migrationMessage, setMigrationMessage] = useState('');
  const [migrationProgress, setMigrationProgress] = useState(0);
  const [lockedProcesses, setLockedProcesses] = useState<string[]>([]);

  // 批量迁移状态
  const [selectedKeys, setSelectedKeys] = useState<Set<string>>(new Set());
  const [batchMigrating, setBatchMigrating] = useState(false);
  const [batchProgress, setBatchProgress] = useState({ current: 0, total: 0 });
  const batchCancelledRef = useRef(false);
  // 批量模式下进程锁弹窗的 Promise resolve 函数
  const batchProcessLockResolveRef = useRef<((value: boolean) => void) | null>(null);
  const [batchWaitingProcessLock, setBatchWaitingProcessLock] = useState(false);

  // 系统文件夹风险确认弹窗（仅 System 类型显示）
  const [riskConfirm, setRiskConfirm] = useState<{
    isOpen: boolean; folder: LargeFolder | null; targetDir: string | null;
  }>({ isOpen: false, folder: null, targetDir: null });

  const { toast, showToast, hideToast } = useToast();

  const { checkBlocked, checkWarning } = useDangerousPathCheck();

  // 页面导航（跳转至设置页）
  const setActiveTab = useContext(TabNavigationContext);

  // 自定义目标选择弹窗（区分 默认 / 自定义 / X 取消 三个操作）
  const [pickerDialog, setPickerDialog] = useState<{
    isOpen: boolean; defaultPath: string; itemName: string;
    resolve: (action: 'default' | 'custom' | null) => void;
  } | null>(null);

  const showTargetPicker = useCallback(
    (defaultPath: string, itemName: string): Promise<'default' | 'custom' | null> =>
      new Promise((resolve) => {
        // 包装 resolve：先清除 dialog 状态再 resolve，避免 isOpen 永为 true 导致死循环
        setPickerDialog({
          isOpen: true, defaultPath, itemName,
          resolve: (action) => { setPickerDialog(null); resolve(action); }
        });
      }),
    [],
  );

  // WARNING 确认弹窗状态（Promise 模式，照抄 pickerDialog）
  const [warningDialog, setWarningDialog] = useState<{
    isOpen: boolean;
    warningInfo: WarningInfo | null;
    resolve: (confirmed: boolean) => void;
  } | null>(null);

  const showWarningDialog = useCallback(
    (warningInfo: WarningInfo): Promise<boolean> =>
      new Promise((resolve) => {
        setWarningDialog({
          isOpen: true, warningInfo,
          resolve: (confirmed) => { setWarningDialog(null); resolve(confirmed); },
        });
      }),
    [],
  );

  // WARNING 用户确认标志，供 handleRiskConfirm 读取
  const [userConfirmedWarning, setUserConfirmedWarning] = useState(false);

  const totalReclaimable = useMemo(
    () => folders.filter(f => !f.is_junction && f.exists).reduce((s, f) => s + f.size, 0),
    [folders],
  );
  const migratedCount = useMemo(() => folders.filter(f => f.is_junction).length, [folders]);

  const fetchFolders = useCallback(async () => {
    try {
      setLoading(true);
      const result = await invoke<LargeFolder[]>('get_large_folders');
      setFolders(result);
      storeApi.setState({ largeFolders: result, largeFoldersLoaded: true });
      // 在前端监听器就绪后启动大小扫描，避免竞态导致事件丢失
      await invoke('start_folder_size_scan', { folders: result });
    } catch (error) {
      console.error('获取大文件夹列表失败:', error);
      showToast('获取文件夹列表失败', 'error');
    } finally { setLoading(false); }
  }, [showToast]);

  // size update events
  useEffect(() => {
    let unlisten: UnlistenFn | null = null;
    (async () => {
      try {
        unlisten = await listen<LargeFolderSizeEvent>('large-folder-size', (event) => {
          const { folder_id, size } = event.payload;
          setFolders(prev => prev.map(f => f.id === folder_id ? { ...f, size } : f));
        });
      } catch { /* ignore */ }
    })();
    return () => { if (unlisten) unlisten(); };
  }, []);

  // 迁移进度事件（复用 engine 层的 migration-progress，与 AppMigration 共享取消标志）
  useEffect(() => {
    let unlisten: UnlistenFn | null = null;
    (async () => {
      try {
        unlisten = await listen<MigrationProgressEvent>('migration-progress', (event) => {
          const data = event.payload;
          setMigrationProgress(data.percent);
          switch (data.step) {
            case 'checking': setMigrationStep('checking'); break;
            case 'counting': setMigrationStep('counting'); break;
            case 'copying': setMigrationStep('copying'); break;
            case 'verifying': setMigrationStep('verifying'); break;
            case 'linking': setMigrationStep('linking'); break;
          }
          setMigrationMessage(data.message);
        });
      } catch { /* ignore */ }
    })();
    return () => { if (unlisten) unlisten(); };
  }, []);

  useEffect(() => {
    if (!visible) return;
    if (storeApi.getState().largeFoldersLoaded) return;
    fetchFolders();
  }, [visible, fetchFolders]);

  async function handleRefresh() { setRefreshing(true); await fetchFolders(); setRefreshing(false); }

  async function openFolder(path: string) {
    try { await invoke('open_folder', { path }); }
    catch (error) { console.error('打开文件夹失败:', error); showToast('打开文件夹失败', 'error'); }
  }

  // 判断文件夹是否已迁移（Junction 状态）
  const isFolderMigrated = useCallback((f: LargeFolder) => f.is_junction, []);

  // 选择/取消选择文件夹：不可用文件夹（!exists）不允许选中
  function handleToggleSelect(folder: LargeFolder) {
    if (!folder.exists) return;
    setSelectedKeys((prev) => {
      const next = new Set(prev);
      if (next.has(folder.id)) next.delete(folder.id);
      else next.add(folder.id);
      return next;
    });
  }

  function handleSelectAll() {
    // 只选可迁移的：未迁移 且 路径存在
    const selectable = folders.filter((f) => !isFolderMigrated(f) && f.exists);
    setSelectedKeys((prev) => {
      if (prev.size === selectable.length && selectable.length > 0) return new Set();
      return new Set(selectable.map((f) => f.id));
    });
  }

  async function handleMigrate(folder: LargeFolder) {
    // 步骤 0: BLOCKED 前置检测（与后端黑名单同步）
    const blockedMsg = checkBlocked(folder.path);
    if (blockedMsg) {
      showToast(blockedMsg, 'error');
      return;
    }

    // 步骤 0.5: WARNING 检测 — 非批量模式弹确认弹窗
    let localConfirmedWarning = false;
    const warningInfo = checkWarning(folder.path);
    if (warningInfo) {
      const confirmed = await showWarningDialog(warningInfo);
      if (!confirmed) return;
      localConfirmedWarning = true;
    }
    setUserConfirmedWarning(localConfirmedWarning);

    // 步骤 1: 进程锁检查
    try {
      const lockResult = await invoke<ProcessLockResult>('check_process_locks', { sourcePath: folder.path });
      if (lockResult.is_locked) {
        const isSystem = folder.folder_type === 'System';
        if (isSystem) {
          // 系统文件夹被占用时仅警告，允许强制继续
          const proceed = await confirm(
            `以下系统进程正在使用 "${folder.display_name}"：\n${lockResult.processes.join(', ')}\n\n强制继续？`,
            { title: '进程占用警告', kind: 'warning', okLabel: '强制继续', cancelLabel: '取消' }
          );
          if (!proceed) return;
        } else {
          showToast(`请先关闭: ${lockResult.processes.join(', ')}`, 'error');
          return;
        }
      }
    } catch { /* 进程检测非关键，失败不阻塞 */ }

    // 步骤 2: 解析迁移目标
    const defaultTarget = loadDataDefaultTarget();
    const targetDir = await resolveDataMigrationTarget(defaultTarget, folder.display_name, setActiveTab, showTargetPicker);
    if (!targetDir) return;

    // 步骤 3: 系统文件夹先弹出风险确认弹窗
    if (folder.folder_type === 'System') {
      setRiskConfirm({ isOpen: true, folder, targetDir });
    } else {
      await startFolderMigration(folder, targetDir, localConfirmedWarning);
    }
  }

  /** 风险确认弹窗确认回调：关闭弹窗后启动迁移 */
  async function handleRiskConfirm() {
    const { folder, targetDir } = riskConfirm;
    if (!folder || !targetDir) return;
    setRiskConfirm({ isOpen: false, folder: null, targetDir: null });
    await startFolderMigration(folder, targetDir, userConfirmedWarning);
  }

  /** 启动文件夹迁移，打开进度弹窗并监听进度事件 */
  async function startFolderMigration(folder: LargeFolder, targetDir: string, userConfirmedWarning: boolean) {
    setMigratingFolder(folder);
    setMigrationModalOpen(true);
    setMigrationStep('checking');
    setMigrationMessage('');
    setMigrationProgress(0);
    setLockedProcesses([]);

    try {
      const result = await invoke<MigrationResult>('migrate_large_folder', {
        sourcePath: folder.path,
        targetDir,
        userConfirmedWarning,
      });

      if (result.success) {
        setMigrationStep('success');
        setMigrationProgress(100);
        setMigrationMessage(result.message);
        showToast('迁移成功！', 'success');
        await fetchFolders();
      } else {
        setMigrationStep('error');
        setMigrationMessage(result.message);
      }
    } catch (error) {
      const errStr = String(error);
      setMigrationStep('error');
      setMigrationMessage(
        errStr.includes('用户取消了迁移')
          ? '迁移已被取消'
          : `迁移过程中发生错误: ${error}`
      );
    }
  }

  /** 取消当前迁移 */
  async function handleCancelMigration() {
    // 进程锁检测阶段（迁移尚未启动）：直接关闭弹窗
    if (migrationStep === 'checking' && lockedProcesses.length > 0) {
      handleCloseMigrationModal();
      return;
    }
    try {
      await invoke('cancel_migration');
      showToast('正在取消迁移...', 'info');
    } catch (error) {
      console.error('取消迁移失败:', error);
    }
  }

  /** 关闭迁移弹窗 */
  function handleCloseMigrationModal() {
    setMigrationModalOpen(false);
    setMigratingFolder(null);
    setMigrationStep('idle');
    setMigrationMessage('');
    setMigrationProgress(0);
    setLockedProcesses([]);
  }

  /** 迁移进行中点击 X → 二次确认后取消 */
  async function handleRequestCloseDuringMigration() {
    // 进程锁检测阶段（迁移尚未启动）：直接关闭弹窗，无需确认
    if (migrationStep === 'checking' && lockedProcesses.length > 0) {
      handleCloseMigrationModal();
      return;
    }

    const confirmed = await confirm(
      '确定要取消当前迁移吗？\n\n已复制的文件将被清理，操作不可撤销。',
      { title: '取消迁移', kind: 'warning', okLabel: '取消迁移', cancelLabel: '继续迁移' }
    );
    if (!confirmed) return;
    try { await invoke('cancel_migration'); } catch { /* ignore */ }
    handleCloseMigrationModal();
  }

  /** 批量迁移：依次迁移每个选中的文件夹 */
  /** 批量迁移：依次迁移每个选中的文件夹，复用单个迁移的进度弹窗 */
  async function handleBatchMigrate() {
    if (selectedKeys.size === 0) return;

    const defaultTarget = loadDataDefaultTarget();
    const targetDir = await resolveDataMigrationTarget(defaultTarget, '批量迁移', setActiveTab, showTargetPicker);
    if (!targetDir) return;

    const selectedFolders = folders.filter((f) => selectedKeys.has(f.id));
    if (selectedFolders.length === 0) return;

    // 含系统文件夹时增加警告
    const hasSystemFolder = selectedFolders.some(f => f.folder_type === 'System');
    const confirmMsg = hasSystemFolder
      ? `即将批量迁移 ${selectedFolders.length} 个文件夹（含系统文件夹）到：\n${targetDir}\n\n⚠ 系统文件夹迁移有风险，请确认已关闭相关应用。\n\n是否继续？`
      : `即将批量迁移 ${selectedFolders.length} 个文件夹到：\n${targetDir}\n\n是否继续？`;

    const confirmed = await confirm(confirmMsg, {
      title: '确认批量迁移',
      kind: hasSystemFolder ? 'warning' : 'info',
      okLabel: '开始迁移',
      cancelLabel: '取消',
    });
    if (!confirmed) return;

    // 初始化批量状态
    setBatchMigrating(true);
    batchCancelledRef.current = false;
    setBatchProgress({ current: 0, total: selectedFolders.length });
    setSelectedKeys(new Set());

    // 打开进度弹窗（复用 MigrationModal）
    setMigrationModalOpen(true);
    setMigrationStep('counting');
    setMigrationMessage('准备开始批量迁移...');
    setMigrationProgress(0);
    setLockedProcesses([]);

    let successCount = 0;
    let failCount = 0;
    const failedFolders: string[] = [];

    for (let i = 0; i < selectedFolders.length; i++) {
      if (batchCancelledRef.current) break;

      const folder = selectedFolders[i];
      setBatchProgress({ current: i + 1, total: selectedFolders.length });

      // 更新弹窗显示当前正在迁移的文件夹
      setMigratingFolder(folder);
      setMigrationStep('checking');
      setMigrationProgress(0);
      setMigrationMessage(`正在处理 (${i + 1}/${selectedFolders.length})...`);

      try {
        // BLOCKED 前端拦截
        const blockedMsg = checkBlocked(folder.path);
        if (blockedMsg) {
          showToast(`${folder.display_name}: ${blockedMsg}`, 'error');
          failCount++;
          failedFolders.push(folder.display_name);
          continue;
        }

        // WARNING 检测 — 批量模式不弹窗，直接跳过
        const warningInfo = checkWarning(folder.path);
        if (warningInfo) {
          showToast(
            `${folder.display_name}: 位于「${warningInfo.category}」(${warningInfo.label})，存在风险，已跳过。\n请在文件夹列表中单独迁移此项以查看详情。`,
            'info'
          );
          failCount++;
          failedFolders.push(`${folder.display_name}（高风险跳过）`);
          continue;
        }

        // 直接调用 migrate_large_folder，后端步骤 0.5 会做准确的占用检测
        const result = await invoke<MigrationResult>('migrate_large_folder', {
          sourcePath: folder.path,
          targetDir,
          userConfirmedWarning: false,
        });

        // 后端步骤 0.5 检测到进程占用 → 暂停批量等待用户介入
        if (!result.success && result.message.includes('检测到以下程序正在运行')) {
          const lines = result.message.split('\n');
          const processLine = lines.find(l => !l.includes('检测到以下程序') && l.trim().length > 0);
          const processes = processLine
            ? processLine.split('、').map(p => p.trim()).filter(Boolean)
            : ['（未知进程）'];

          setLockedProcesses(processes);
          setMigrationStep('checking');
          setMigrationMessage(`${folder.display_name} 有进程占用，请处理后选择继续或跳过`);

          setBatchWaitingProcessLock(true);
          const shouldContinue = await new Promise<boolean>((resolve) => {
            batchProcessLockResolveRef.current = resolve;
          });
          batchProcessLockResolveRef.current = null;
          setBatchWaitingProcessLock(false);

          setLockedProcesses([]);
          setMigrationStep('checking');

          if (!shouldContinue || batchCancelledRef.current) {
            batchCancelledRef.current = true;
            break;
          }
          showToast(`${folder.display_name}: 存在进程占用，已跳过`, 'info');
          failCount++;
          failedFolders.push(`${folder.display_name}（进程占用跳过）`);
          continue;
        }

        // 处理 TARGET_EXISTS 特殊返回值（不能直接 Toast 路径字符串给用户）
        if (!result.success && (
          result.message.startsWith('TARGET_EXISTS_RETRY:') ||
          result.message.startsWith('TARGET_EXISTS:')
        )) {
          if (batchCancelledRef.current) { failCount++; failedFolders.push(folder.display_name); continue; }
          const isRetry = result.message.startsWith('TARGET_EXISTS_RETRY:');
          const existingPath = result.message.replace(/^TARGET_EXISTS(?:_RETRY)?:/, '');
          const promptMsg = isRetry
            ? `${folder.display_name} 上次迁移未完成，目标位置存在残留目录：\n${existingPath}\n\n覆盖将清理残留并重新迁移。`
            : `${folder.display_name} 的目标路径已存在：\n${existingPath}\n\n是否覆盖？`;
          const overwrite = await confirm(promptMsg, {
            title: '目标目录已存在',
            kind: 'warning',
            okLabel: '覆盖并迁移',
            cancelLabel: '跳过',
          });
          if (overwrite) {
            // 以 force_overwrite 重试
            const retryResult = await invoke<MigrationResult>('migrate_large_folder', {
              sourcePath: folder.path,
              targetDir,
              forceOverwrite: true,
              userConfirmedWarning: false,
            });
            if (retryResult.success) {
              successCount++;
            } else {
              showToast(`${folder.display_name}: ${retryResult.message}`, 'error');
              failCount++;
              failedFolders.push(folder.display_name);
            }
          } else {
            failCount++;
            failedFolders.push(folder.display_name);
          }
          continue;
        }

        if (result.success) {
          successCount++;
        } else {
          // 后端错误消息（文件占用、空间不足等），直接显示
          showToast(`${folder.display_name}: ${result.message}`, 'error');
          failCount++;
          failedFolders.push(folder.display_name);
        }
      } catch (error) {
        const errStr = String(error);
        if (!errStr.includes('用户取消了迁移')) {
          showToast(`${folder.display_name}: ${error}`, 'error');
          failedFolders.push(folder.display_name);
        }
        failCount++;
      }
    }

    setBatchMigrating(false);
    setBatchProgress({ current: 0, total: 0 });
    handleCloseMigrationModal();

    if (batchCancelledRef.current) {
      showToast(`批量迁移已停止，已完成 ${successCount} 个`, 'info');
    } else if (failCount === 0) {
      showToast(`批量迁移完成：${successCount} 个全部成功`, 'success');
    } else {
      showToast(
        `批量迁移完成：${successCount} 成功，${failCount} 失败` +
        (failedFolders.length > 0 ? `\n失败：${failedFolders.join('、')}` : ''),
        'info'
      );
    }

    await fetchFolders();
  }

  /** 停止批量迁移 */
  function handleStopBatchMigrate() {
    batchCancelledRef.current = true;
    invoke('cancel_migration').catch(() => {});
  }

  // 批量迁移进程锁：跳过当前文件夹，继续批量
  function handleBatchProcessLockSkip() {
    batchProcessLockResolveRef.current?.(true);
  }
  // 批量迁移进程锁：停止批量迁移
  function handleBatchProcessLockStop() {
    batchProcessLockResolveRef.current?.(false);
  }

  async function handleRestore(folder: LargeFolder) {
    if (folder.app_process_names.length > 0) {
      try {
        const lockResult = await invoke<ProcessLockResult>('check_process_locks', { sourcePath: folder.path });
        if (lockResult.is_locked) {
          showToast(`请先关闭: ${lockResult.processes.join(', ')}`, 'error');
          return;
        }
      } catch { /* non-critical */ }
    }
    setRestoringFolderId(folder.id);
    setRestoreProgressMap(prev => ({ ...prev, [folder.id]: 0 }));
    let unlisten: UnlistenFn | null = null;
    try {
      try {
        unlisten = await listen<MigrationProgressEvent>('migration-progress', (event) => {
          const data = event.payload;
          // 恢复事件以原路径作为 task_id，只更新当前文件夹按钮。
          if (data.task_id.toLowerCase() === folder.path.toLowerCase()) {
            setRestoreProgressMap(prev => ({ ...prev, [folder.id]: data.percent }));
          }
        });
      } catch { /* 监听失败不阻断恢复，按钮会保持 0% */ }
      const result = await invoke<MigrationResult>('restore_large_folder', { junctionPath: folder.path });
      if (result.success) {
        showToast(result.message, 'success');
        await fetchFolders();
      } else {
        showToast(result.message, 'error');
      }
    } catch (error) {
      showToast(`恢复失败: ${error}`, 'error');
    } finally {
      if (unlisten) unlisten();
      setRestoreProgressMap(prev => {
        const next = { ...prev };
        delete next[folder.id];
        return next;
      });
      setRestoringFolderId(null);
    }
  }

  async function handleAddCustomFolder() {
    const selectedPath = await open({ directory: true, title: '选择要监控的文件夹' });
    if (!selectedPath) return;
    // 危险路径检测：拦截系统目录、浏览器安装目录、GPU 驱动目录
    const dangerMsg = checkBlocked(selectedPath as string);
    if (dangerMsg) {
      showToast(dangerMsg, 'error');
      return;
    }
    try {
      await invoke('add_custom_folder', { path: selectedPath as string });
      showToast('文件夹已添加', 'success');
      await fetchFolders();
    } catch (error) { showToast(`添加失败: ${error}`, 'error'); }
  }

  async function handleRemoveCustomFolder(folder: LargeFolder) {
    try {
      await invoke('remove_custom_folder', { id: folder.id });
      showToast(`已移除 "${folder.display_name}"`, 'success');
      await fetchFolders();
    } catch (error) { showToast(`移除失败: ${error}`, 'error'); }
  }

  const systemFolders = folders.filter(f => f.folder_type === 'System');
  const appDataFolders = folders.filter(f => f.folder_type === 'AppData');
  const customFolders = folders.filter(f => f.folder_type === 'Custom');

  return (
    <div className="h-full overflow-hidden flex flex-col" style={{ padding: '12px 16px' }}>
      <div className="h-full flex flex-col w-full gap-3">
        {/* top stats + actions */}
        <div className="flex items-center justify-between flex-shrink-0"
          style={{ paddingBottom: '10px', borderBottom: '1px solid var(--border-color)' }}>
          <div className="flex items-center gap-4 text-[12px]">
            <span style={{ color: 'var(--text-secondary)' }}>
              可释放 <strong style={{ color: 'var(--text-primary)' }}>{loading ? '...' : formatSize(totalReclaimable)}</strong>
            </span>
            {migratedCount > 0 && (
              <span style={{ color: 'var(--text-secondary)' }}>
                已迁移 <strong style={{ color: 'var(--color-success)' }}>{migratedCount}</strong> 个
              </span>
            )}
            {/* 批量操作 */}
            <button
              onClick={handleSelectAll}
              className="text-[11px] cursor-pointer"
              style={{ color: 'var(--color-primary)', background: 'none', border: 'none' }}
            >
              {selectedKeys.size > 0 ? '取消全选' : '全选未迁移'}
            </button>
            {batchMigrating ? (
              <button
                onClick={handleStopBatchMigrate}
                className="btn btn-sm h-7 text-[11px]"
                style={{ background: 'var(--color-danger)', color: 'var(--text-inverse)', borderColor: 'var(--color-danger)' }}
              >
                停止 ({batchProgress.current}/{batchProgress.total})
              </button>
            ) : selectedKeys.size > 0 ? (
              <button
                onClick={handleBatchMigrate}
                className="btn btn-primary btn-sm h-7 text-[11px]"
              >
                <ArrowRightLeft className="w-3.5 h-3.5" />
                批量迁移 ({selectedKeys.size})
              </button>
            ) : null}
          </div>
          <div className="flex items-center gap-2">
            <button onClick={handleAddCustomFolder} className="btn h-7 text-[12px]">
              <Plus className="w-3.5 h-3.5" />
              添加文件夹
            </button>
            <button onClick={handleRefresh} disabled={refreshing} className="btn h-7 text-[12px]">
              <RefreshCw className={`w-3.5 h-3.5 ${refreshing ? 'animate-spin' : ''}`} />
              刷新
            </button>
          </div>
        </div>

        {/* content */}
        <div className="flex-1 min-h-0 overflow-y-auto">
          {loading ? (
            <div className="flex flex-col">
              {[1,2,3,4,5].map(i => (
                <div key={i} className="flex items-center gap-3 animate-pulse" style={{ height: 'var(--row-height)', padding: '0 8px', borderBottom: '1px solid var(--border-color)' }}>
                  <div className="w-7 h-7 rounded" style={{ background: 'var(--bg-row-hover)' }} />
                  <div className="flex-1 h-3 rounded" style={{ background: 'var(--bg-row-hover)' }} />
                  <div className="w-20 h-3 rounded" style={{ background: 'var(--bg-row-hover)' }} />
                  <div className="w-16 h-7 rounded" style={{ background: 'var(--bg-row-hover)' }} />
                </div>
              ))}
            </div>
          ) : folders.length === 0 ? (
            <EmptyState icon={<FolderOpen />} title="未检测到可迁移的文件夹" description="系统扫描未发现可管理的数据目录" />
          ) : (
            <div className="flex flex-col gap-4">
              {systemFolders.length > 0 && (
                <section>
                  <div className="flex items-center gap-2 mb-1.5 px-2">
                    <span className="text-[12px] font-semibold" style={{ color: 'var(--text-primary)' }}>系统文件夹</span>
                    <span className="badge" style={{ color: 'var(--color-warning)', background: 'var(--color-warning-light)' }}>
                      <AlertTriangle className="w-3 h-3" />谨慎操作
                    </span>
                  </div>
                  {systemFolders.map(f => (
                    <FolderRow key={f.id} folder={f} onMigrate={handleMigrate} onRestore={handleRestore}
                      onOpenFolder={openFolder} isMigrating={migratingFolder?.id === f.id} isRestoring={restoringFolderId === f.id}
                      restoreProgress={restoreProgressMap[f.id]}
                      showCheckbox isSelected={selectedKeys.has(f.id)} onToggleSelect={handleToggleSelect} />
                  ))}
                </section>
              )}

              {appDataFolders.length > 0 && (
                <section>
                  <div className="px-2 mb-1.5">
                    <span className="text-[12px] font-semibold" style={{ color: 'var(--text-primary)' }}>应用数据</span>
                  </div>
                  {appDataFolders.map(f => (
                    <FolderRow key={f.id} folder={f} onMigrate={handleMigrate} onRestore={handleRestore}
                      onOpenFolder={openFolder} isMigrating={migratingFolder?.id === f.id} isRestoring={restoringFolderId === f.id}
                      restoreProgress={restoreProgressMap[f.id]}
                      showCheckbox isSelected={selectedKeys.has(f.id)} onToggleSelect={handleToggleSelect} />
                  ))}
                </section>
              )}

              {customFolders.length > 0 && (
                <section>
                  <div className="px-2 mb-1.5">
                    <span className="text-[12px] font-semibold" style={{ color: 'var(--text-primary)' }}>自定义文件夹</span>
                  </div>
                  {customFolders.map(f => (
                    <FolderRow key={f.id} folder={f} onMigrate={handleMigrate} onRestore={handleRestore}
                      onOpenFolder={openFolder} onRemove={handleRemoveCustomFolder}
                      isMigrating={migratingFolder?.id === f.id} isRestoring={restoringFolderId === f.id}
                      restoreProgress={restoreProgressMap[f.id]}
                      showCheckbox isSelected={selectedKeys.has(f.id)} onToggleSelect={handleToggleSelect} />
                  ))}
                </section>
              )}
            </div>
          )}
        </div>
      </div>

      <RiskConfirmModal isOpen={riskConfirm.isOpen} folder={riskConfirm.folder}
        onConfirm={handleRiskConfirm}
        onCancel={() => setRiskConfirm({ isOpen: false, folder: null, targetDir: null })} />

      {/* WARNING 危险路径确认弹窗 */}
      {warningDialog && (
        <WarningConfirmDialog
          isOpen={warningDialog.isOpen}
          warningInfo={warningDialog.warningInfo}
          onConfirm={() => warningDialog.resolve(true)}
          onCancel={() => warningDialog.resolve(false)}
        />
      )}

      {/* 迁移目标选择弹窗（区分 默认 / 自定义 / 取消） */}
      {pickerDialog && (
        <TargetPickerDialog
          isOpen={pickerDialog.isOpen}
          title="迁移目录"
          defaultPath={pickerDialog.defaultPath}
          itemName={pickerDialog.itemName}
          onUseDefault={() => pickerDialog.resolve('default')}
          onUseCustom={() => pickerDialog.resolve('custom')}
          onClose={() => pickerDialog.resolve(null)}
        />
      )}

      {/* 迁移进度弹窗 */}
      <MigrationModal
        isOpen={migrationModalOpen}
        step={migrationStep}
        appName={
          batchMigrating && batchProgress.total > 0
            ? `批量迁移 (${batchProgress.current}/${batchProgress.total}) — ${migratingFolder?.display_name || ''}`
            : migratingFolder?.display_name || ''
        }
        message={migrationMessage}
        lockedProcesses={lockedProcesses}
        progress={migrationProgress}
        title="数据迁移"
        onCancel={
          batchMigrating
            ? (batchWaitingProcessLock
                ? handleBatchProcessLockStop
                : handleStopBatchMigrate)
            : handleCancelMigration
        }
        onForceContinue={
          batchWaitingProcessLock
            ? handleBatchProcessLockSkip
            : undefined
        }
        onClose={handleCloseMigrationModal}
        onRequestClose={batchMigrating
          ? async () => { handleStopBatchMigrate(); }
          : handleRequestCloseDuringMigration
        }
      />

      <Toast message={toast.message} type={toast.type} visible={toast.visible} onClose={hideToast} />
    </div>
  );
}
