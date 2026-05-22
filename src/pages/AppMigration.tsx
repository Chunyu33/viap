// 应用迁移页面
// 实现完整的迁移流程：目录选择 -> 进程检测 -> 文件复制 -> 创建链接
// 支持真实进度上报和取消操作

import { useEffect, useState, useTransition, useCallback, useContext, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { confirm, open } from '@tauri-apps/plugin-dialog';
import AppList from '../components/AppList';
import MigrationModal from '../components/MigrationModal';
import CleanupModal from '../components/CleanupModal';
import TargetPickerDialog from '../components/TargetPickerDialog';
import Toast, { useToast } from '../components/Toast';
import { logger } from '../utils/logger';
import { TabNavigationContext } from '../App';
import {
  CleanupResult,
  InstalledApp,
  LeftoverItem,
  MigrationProgressEvent,
  MigrationRecord,
  MigrationResult,
  MigrationStep,
  ProcessLockResult,
  TabType,
  UninstallPreview,
  UninstallResult,
} from '../types';

// 模块级大小缓存：Tab 切换后无需重新遍历磁盘获取目录大小
// 仅当应用列表变更（卸载/迁移/手动刷新）时才重新计算
let cachedSizeMap: Map<string, number> | null = null;

/** 从 localStorage 读取默认应用迁移目录，仅非 C 盘路径有效 */
function loadAppDefaultTarget(): string | null {
  try {
    const saved = JSON.parse(localStorage.getItem('viap_settings') || '{}');
    const path = saved.defaultAppTargetPath;
    if (path && typeof path === 'string' && path.length > 0) {
      // C 盘路径视为无效，需由用户重新选择
      if (path.startsWith('C:') || path.startsWith('c:')) return null;
      return path;
    }
  } catch { /* 设置读取失败时忽略 */ }
  return null;
}

/**
 * 解析迁移目录目录：优先使用默认设置，否则引导配置或手动选择
 * 返回选中的目标路径，null 表示用户取消操作
 *
 * 使用自定义 TargetPickerDialog 替代原生 confirm 弹窗，
 * 以区分「使用默认」「自定义目录」和「X 关闭」三个独立操作。
 */
async function resolveMigrationTarget(
  defaultPath: string | null,
  appName: string,
  navigateToSettings: ((tab: TabType) => void) | null,
  showTargetPicker: (defaultPath: string, itemName: string) => Promise<'default' | 'custom' | null>,
): Promise<string | null> {
  if (defaultPath) {
    const action = await showTargetPicker(defaultPath, `应用 "${appName}" 将迁移到此目录`);
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
    title: `选择迁移目录文件夹 - ${appName}`,
  });
  return targetDir as string | null;
}

export default function AppMigration() {
  const [apps, setApps] = useState<InstalledApp[]>([]);
  const [appsLoading, setAppsLoading] = useState(true);
  const [sizesLoading, setSizesLoading] = useState(false);
  const [sizeMap, setSizeMap] = useState<Map<string, number>>(cachedSizeMap ?? new Map());
  const [refreshing, setRefreshing] = useState(false);

  // 将应用列表相关的状态更新标记为低优先级，避免阻塞用户交互
  const [, startTransition] = useTransition();

  // 已迁移的路径列表
  const [migratedPaths, setMigratedPaths] = useState<string[]>([]);
  // 应用迁移记录（用于还原时获取 historyId）
  const [appMigrationRecords, setAppMigrationRecords] = useState<MigrationRecord[]>([]);

  // 迁移状态
  const [migrationModalOpen, setMigrationModalOpen] = useState(false);
  const [migrationStep, setMigrationStep] = useState<MigrationStep>('idle');
  const [migratingApp, setMigratingApp] = useState<InstalledApp | null>(null);
  const [migrationMessage, setMigrationMessage] = useState('');
  const [migrationProgress, setMigrationProgress] = useState(0);
  const [lockedProcesses, setLockedProcesses] = useState<string[]>([]);
  // 进程锁检测后保存已选目标目录，避免强制继续时重新弹窗选择
  const [pendingTargetDir, setPendingTargetDir] = useState<string | null>(null);

  // 强力卸载状态
  const [uninstallingKey, setUninstallingKey] = useState<string | null>(null);
  // 还原状态
  const [restoringKey, setRestoringKey] = useState<string | null>(null);
  // 批量迁移
  const [selectedKeys, setSelectedKeys] = useState<Set<string>>(new Set());
  const [batchMigrating, setBatchMigrating] = useState(false);
  const [batchProgress, setBatchProgress] = useState({ current: 0, total: 0 });
  // 批量迁移取消标志，用 ref 避免异步循环中闭包捕获过期 state
  const batchCancelledRef = useRef(false);
  const [cleanupModalOpen, setCleanupModalOpen] = useState(false);
  const [cleanupTargetAppName, setCleanupTargetAppName] = useState('');
  const [cleanupTargetPublisher, setCleanupTargetPublisher] = useState<string | null>(null);
  const [leftoverItems, setLeftoverItems] = useState<LeftoverItem[]>([]);
  const [cleanupLoading, setCleanupLoading] = useState(false);
  const [scanningResidue, setScanningResidue] = useState(false);

  // Toast 通知
  const { toast, showToast, hideToast } = useToast();

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

  // 打开应用所在目录，失败时通过 Toast 反馈
  async function handleOpenFolder(app: InstalledApp) {
    try {
      await invoke('open_folder', { path: app.install_location });
    } catch (error) {
      showToast(`打开目录失败：${error}`, 'error');
    }
  }

  // 手动触发残留扫描（先打开弹窗展示扫描状态，再执行扫描）
  async function handleScanResidue(app: InstalledApp) {
    // 先打开弹窗进入扫描状态
    setCleanupTargetAppName(app.display_name);
    setCleanupTargetPublisher(app.publisher || null);
    setLeftoverItems([]);
    setScanningResidue(true);
    setCleanupModalOpen(true);

    try {
      const leftovers = await invoke<LeftoverItem[]>('scan_app_residue', {
        appName: app.display_name,
        publisher: app.publisher || null,
        installLocation: app.install_location || null,
      });

      setScanningResidue(false);
      setLeftoverItems(leftovers);

      if (leftovers.length === 0) {
        setCleanupModalOpen(false);
        showToast(`${app.display_name} 未检测到残留`, 'success');
        await handleRefresh();
      }
    } catch (error) {
      setScanningResidue(false);
      setCleanupModalOpen(false);
      showToast(`残留扫描失败: ${error}`, 'error');
    }
  }

  async function fetchInstalledApps() {
    try {
      setAppsLoading(true);
      const installedApps = await invoke<InstalledApp[]>('get_installed_apps');
      startTransition(() => setApps(installedApps));
      // 后台异步加载目录大小，不阻塞 UI
      loadAppSizes(installedApps);
    } catch (error) {
      logger.error('获取应用列表失败:', error);
      startTransition(() => setApps([]));
    } finally {
      setAppsLoading(false);
    }
  }

  // 分批异步获取所有应用的目录大小
  // 命中模块级缓存时跳过全部 IO，实现 Tab 切回"秒开"
  async function loadAppSizes(appList: InstalledApp[]) {
    // 检查模块级缓存：key 集合一致则直接复用，零 IO 开销
    if (cachedSizeMap && cachedSizeMap.size > 0) {
      const currentKeys = new Set(appList.map(a => a.registry_path || a.install_location));
      const cachedKeys = new Set(cachedSizeMap.keys());
      if (currentKeys.size === cachedKeys.size && [...currentKeys].every(k => cachedKeys.has(k))) {
        // 缓存命中，跳过全部磁盘遍历
        startTransition(() => {
          setSizeMap(new Map(cachedSizeMap!));
        });
        return;
      }
    }

    setSizesLoading(true);
    const batchSize = 8;
    const localSizeMap = new Map<string, number>();

    for (let i = 0; i < appList.length; i += batchSize) {
      const batch = appList.slice(i, i + batchSize);
      const results = await Promise.allSettled(
        batch.map(app => invoke<number>('get_app_size', { installLocation: app.install_location }))
      );
      for (let j = 0; j < batch.length; j++) {
        const key = batch[j].registry_path || batch[j].install_location;
        const result = results[j];
        if (result.status === 'fulfilled') {
          localSizeMap.set(key, result.value);
        }
      }
    }

    // 写入模块级缓存 + React 状态
    cachedSizeMap = new Map(localSizeMap);
    startTransition(() => {
      setSizeMap(new Map(localSizeMap));
      setSizesLoading(false);
    });
  }

  // 获取应用迁移记录，并同步已迁移路径
  async function fetchAppMigrationRecords() {
    try {
      const records = await invoke<MigrationRecord[]>('get_migration_history');
      const appRecords = records.filter(record => record.record_type === 'App');
      setAppMigrationRecords(appRecords);
      setMigratedPaths(appRecords.map(record => record.original_path));
    } catch (error) {
      logger.error('获取应用迁移记录失败:', error);
      setAppMigrationRecords([]);
      setMigratedPaths([]);
    }
  }

  async function handleRefresh() {
    cachedSizeMap = null; // 应用列表已变更（迁移/卸载），强制重新计算大小
    // 使用 refresh_apps 绕过缓存强刷注册表扫描，确保迁移后的目录联接路径与迁移记录一致
    try {
      const freshApps = await invoke<InstalledApp[]>('refresh_apps');
      startTransition(() => setApps(freshApps));
      loadAppSizes(freshApps);
    } catch (error) {
      logger.error('刷新应用列表失败:', error);
    }
    await fetchAppMigrationRecords();
  }

  // 手动刷新：清空后端缓存 + 前端大小缓存，强制全量扫描
  async function handleRefreshApps() {
    setRefreshing(true);
    cachedSizeMap = null; // 重置前端大小缓存
    try {
      const freshApps = await invoke<InstalledApp[]>('refresh_apps');
      startTransition(() => setApps(freshApps));
      // 刷新后重新加载大小，但迁移记录无需重刷
      loadAppSizes(freshApps);
      await fetchAppMigrationRecords();
    } catch (error) {
      logger.error('刷新应用列表失败:', error);
    } finally {
      setRefreshing(false);
    }
  }

  // 还原流程：将已迁移应用恢复到原始位置
  async function handleRestore(app: InstalledApp) {
    const record = appMigrationRecords.find(r =>
      r.original_path.toLowerCase() === app.install_location.toLowerCase()
    );

    if (!record) {
      showToast('未找到该应用的迁移记录，无法执行还原', 'error');
      return;
    }

    const currentRestoreKey = `${app.display_name}|${app.registry_path}`;

    try {
      setRestoringKey(currentRestoreKey);

      const result = await invoke<MigrationResult>('restore_app', {
        historyId: record.id,
      });

      if (result.success) {
        showToast(`${app.display_name} 已成功还原`, 'success');
        await handleRefresh();
      } else {
        showToast(result.message || '还原失败', 'error');
      }
    } catch (error) {
      showToast(`还原失败: ${error}`, 'error');
    } finally {
      setRestoringKey(null);
    }
  }

  // 强制删除 + 残留扫描流程（供预览失败和卸载失败两处复用）
  async function forceRemoveApp(app: InstalledApp, useRecycleBin: boolean) {
    const currentUninstallKey = `${app.display_name}|${app.registry_path}`;
    try {
      setUninstallingKey(currentUninstallKey);
      const result = await invoke<UninstallResult>('force_remove_application', {
        input: { app_id: app.display_name, registry_path: app.registry_path, install_location: app.install_location, use_recycle_bin: useRecycleBin },
      });
      if (result.success) {
        showToast(result.message, 'success');
        const confirmScan = await confirm(
          `${app.display_name} 强制删除完成。\n\n是否扫描残留文件？`,
          { title: '扫描残留', kind: 'warning', okLabel: '开始扫描', cancelLabel: '稍后再说' }
        );
        if (confirmScan) {
          await handleScanResidue(app);
        } else {
          await handleRefresh();
        }
      } else {
        showToast(result.message || '强制删除失败', 'error');
      }
    } catch (error) {
      showToast(`强制删除失败: ${error}`, 'error');
    } finally {
      setUninstallingKey(null);
    }
  }

  // 强力卸载流程
  async function handleUninstall(app: InstalledApp) {
    // 读取用户设置的删除方式（默认移入回收站）
    let useRecycleBin = true;
    try {
      const saved = JSON.parse(localStorage.getItem('viap_settings') || '{}');
      useRecycleBin = saved.useRecycleBin !== false;
    } catch { /* use default */ }

    // 先预览卸载命令
    let previewCommands: string[] = [];
    let previewFailed = false;
    try {
      const preview = await invoke<UninstallPreview>('preview_uninstall', {
        input: {
          app_id: app.display_name,
          registry_path: app.registry_path,
          install_location: app.install_location,
          use_recycle_bin: useRecycleBin,
        },
      });
      previewCommands = preview.commands;
    } catch {
      previewFailed = true;
    }

    const currentUninstallKey = `${app.display_name}|${app.registry_path}`;

    // 卸载程序不可用（损坏/缺失）→ 走强制删除流程
    if (previewFailed || previewCommands.length === 0) {
      const forceConfirm = await confirm(
        `${app.display_name} 的卸载程序不可用（可能已损坏或被删除）。\n\n是否执行强制删除？将直接移除安装目录和注册表项。`,
        { title: '强制删除', kind: 'warning', okLabel: '强制删除', cancelLabel: '取消' }
      );
      if (!forceConfirm) return;
      await forceRemoveApp(app, useRecycleBin);
      return;
    }

    // 正常卸载流程
    const commandLines = `\n\n即将执行的卸载命令：\n${previewCommands.map((c, i) => `  ${i + 1}. ${c}`).join('\n')}`;

    const confirmed = await confirm(
      `即将启动 ${app.display_name} 的卸载程序。\n\n此操作可能删除应用及其相关组件，是否继续？${commandLines}`,
      { title: '确认强力卸载', kind: 'warning', okLabel: '继续卸载', cancelLabel: '取消' }
    );
    if (!confirmed) return;

    try {
      setUninstallingKey(currentUninstallKey);

      const result = await invoke<UninstallResult>('uninstall_application', {
        input: { app_id: app.display_name, registry_path: app.registry_path, install_location: app.install_location, use_recycle_bin: useRecycleBin },
      });

      if (result.success) {
        showToast(result.message || `${app.display_name} 卸载流程已完成`, 'success');

        const confirmScan = await confirm(
          `${app.display_name} 卸载流程已结束。\n\n是否现在开始残留扫描？（建议在卸载向导完全关闭后执行）`,
          {
            title: '手动确认残留扫描',
            kind: 'warning',
            okLabel: '开始扫描',
            cancelLabel: '稍后再说',
          }
        );

        if (confirmScan) {
          await handleScanResidue(app);
        } else {
          await handleRefresh();
        }
      } else {
        showToast(result.message || '启动卸载失败', 'error');
      }
    } catch (error) {
      const errStr = String(error);
      // 卸载命令已执行但注册表仍检测到应用（卸载向导未确认完成）
      // 或所有卸载命令均执行失败 → 引导用户转用强制删除
      if (errStr.includes('仍检测到应用存在') || errStr.includes('卸载命令执行失败')) {
        const forceConfirm = await confirm(
          `${app.display_name} 卸载未完成。\n\n${errStr}\n\n是否转用强制删除？将直接移除安装目录和注册表项。`,
          { title: '卸载未完成', kind: 'warning', okLabel: '强制删除', cancelLabel: '取消' }
        );
        if (forceConfirm) {
          await forceRemoveApp(app, useRecycleBin);
        }
      } else {
        showToast(`卸载未完成：${error}`, 'error');
      }
    } finally {
      setUninstallingKey(null);
    }
  }

  // 切换残留项选中状态
  function handleToggleLeftover(path: string) {
    setLeftoverItems((prev) =>
      prev.map((item) =>
        item.path === path
          ? { ...item, selected: !item.selected }
          : item
      )
    );
  }

  // 执行清理
  async function handleConfirmCleanup() {
    const selectedPaths = leftoverItems
      .filter((item) => item.selected)
      .map((item) => item.path);

    if (selectedPaths.length === 0) {
      showToast('请至少选择一项残留再进行清理', 'error');
      return;
    }

    try {
      setCleanupLoading(true);
      const result = await invoke<CleanupResult>('execute_cleanup', {
        items: selectedPaths,
        appName: cleanupTargetAppName || null,
        publisher: cleanupTargetPublisher,
      });

      if (result.success) {
        showToast('清理成功', 'success');
      } else {
        showToast(result.message || '部分项目清理失败，请重试', 'error');
      }

      setCleanupModalOpen(false);
      setLeftoverItems([]);
      setCleanupTargetAppName('');
      setCleanupTargetPublisher(null);
      await handleRefresh();
    } catch (error) {
      showToast(`执行清理失败: ${error}`, 'error');
    } finally {
      setCleanupLoading(false);
    }
  }

  // 关闭清理弹窗
  function handleCloseCleanupModal() {
    if (cleanupLoading || scanningResidue) {
      return;
    }

    setCleanupModalOpen(false);
    setLeftoverItems([]);
    setCleanupTargetAppName('');
    setCleanupTargetPublisher(null);
    setScanningResidue(false);
  }

  // 核心迁移流程
  async function handleMigrate(app: InstalledApp) {
    // 步骤 1: 解析迁移目录（默认设置 / 引导设置 / 手动选择）
    const defaultTarget = loadAppDefaultTarget();
    const targetDir = await resolveMigrationTarget(defaultTarget, app.display_name, setActiveTab, showTargetPicker);
    if (!targetDir) return;

    // 初始化迁移状态
    setMigratingApp(app);
    setMigrationModalOpen(true);
    setMigrationStep('checking');
    setMigrationMessage('');
    setMigrationProgress(0);
    setLockedProcesses([]);

    // 步骤 2: 检查进程锁
    try {
      const lockResult = await invoke<ProcessLockResult>('check_process_locks', {
        sourcePath: app.install_location,
      });

      if (lockResult.is_locked) {
        // 保存已选目标目录，供强制继续时直接复用，不再弹窗
        setPendingTargetDir(targetDir as string);
        setLockedProcesses(lockResult.processes);
        return;
      }

      // 无进程占用，直接开始复制
      await startCopyPhase(app, targetDir as string);
    } catch (error) {
      setMigrationStep('error');
      setMigrationMessage(`检测进程锁失败: ${error}`);
    }
  }

  // 用户确认强制继续（忽略进程锁），复用已保存的目标目录
  async function handleForceContinue() {
    if (!migratingApp || !pendingTargetDir) return;
    setLockedProcesses([]);
    const targetDir = pendingTargetDir;
    setPendingTargetDir(null);
    await startCopyPhase(migratingApp, targetDir);
  }

  // 开始文件复制阶段（带事件监听）
  async function startCopyPhase(app: InstalledApp, targetDir: string) {
    setMigrationStep('counting');
    setLockedProcesses([]);

    // 注册进度事件监听器
    let unlisten: UnlistenFn | null = null;
    try {
      unlisten = await listen<MigrationProgressEvent>('migration-progress', (event) => {
        const data = event.payload;
        setMigrationProgress(data.percent);

        // 根据后端 step 同步前端步骤
        switch (data.step) {
          case 'counting':
            setMigrationStep('counting');
            break;
          case 'copying':
            setMigrationStep('copying');
            break;
          case 'verifying':
            setMigrationStep('verifying');
            break;
          case 'linking':
            setMigrationStep('linking');
            break;
          case 'done':
            // 不在这里处理，等待 migrate_app 返回
            break;
        }
        setMigrationMessage(data.message);
      });
    } catch (error) {
      logger.error('注册进度监听失败:', error);
    }

    // 执行迁移（Rust 后端会在复制过程中推送进度事件）
    // 支持 TARGET_EXISTS 重试：检测到残留目录时弹出确认框，用户确认后以 force_overwrite 重试
    try {
      let result = await invoke<MigrationResult>('migrate_app', {
        appName: app.display_name,
        source: app.install_location,
        targetParent: targetDir,
      });

      // 目标路径已有残留目录 → 询问用户是否覆盖
      if (!result.success && (result.message.startsWith('TARGET_EXISTS_RETRY:') || result.message.startsWith('TARGET_EXISTS:'))) {
        const isRetry = result.message.startsWith('TARGET_EXISTS_RETRY:');
        const existingPath = isRetry
          ? result.message.replace('TARGET_EXISTS_RETRY:', '')
          : result.message.replace('TARGET_EXISTS:', '');
        const promptMsg = isRetry
          ? `上次迁移未完全完成，目标位置存在残留目录：\n${existingPath}\n\n覆盖将清理残留并重新迁移。`
          : `目标路径已存在残留目录：\n${existingPath}\n\n可能是上次恢复或迁移失败留下的。\n覆盖将删除该目录后重新迁移，是否继续？`;
        const overwrite = await confirm(
          promptMsg,
          { title: '目标目录已存在', kind: 'warning', okLabel: '覆盖并迁移', cancelLabel: '取消' }
        );
        if (!overwrite) {
          if (unlisten) unlisten();
          setMigrationStep('error');
          setMigrationMessage('用户取消了迁移（目标路径已存在残留目录）');
          return;
        }
        // 用户确认覆盖，使用 force_overwrite 重试
        result = await invoke<MigrationResult>('migrate_app', {
          appName: app.display_name,
          source: app.install_location,
          targetParent: targetDir,
          forceOverwrite: true,
        });
      }

      // 源路径是 Junction 且指向目标：恢复失败残留状态，覆盖迁移会丢失数据
      if (!result.success && result.message.startsWith('JUNCTION_LOOP:')) {
        const targetPath = result.message.replace('JUNCTION_LOOP:', '');
        if (unlisten) unlisten();
        setMigrationStep('error');
        setMigrationMessage(
          `检测到原路径仍是指向目标盘的链接，无法覆盖迁移。\n\n` +
          `请先前往「迁移记录」页面恢复该应用，再重新迁移。\n\n` +
          `目标位置：${targetPath}`
        );
        return;
      }

      // 取消事件监听
      if (unlisten) unlisten();

      if (result.success) {
        setMigrationStep('success');
        setMigrationProgress(100);
        setMigrationMessage(result.message);
        showToast('迁移成功！', 'success');
        await handleRefresh();
      } else {
        setMigrationStep('error');
        setMigrationMessage(result.message);
      }
    } catch (error) {
      if (unlisten) unlisten();
      setMigrationStep('error');
      // 区分用户取消和真实错误
      const errStr = String(error);
      setMigrationMessage(
        errStr.includes('用户取消了迁移')
          ? '迁移已被取消'
          : `迁移过程中发生错误: ${error}`
      );
    }
  }

  // 取消当前迁移
  async function handleCancelMigration() {
    // 进程锁检测阶段（迁移尚未启动）：直接关闭弹窗，无需通知后端
    if (migrationStep === 'checking' && lockedProcesses.length > 0) {
      handleCloseMigrationModal();
      return;
    }
    // 迁移进行中：通知后端取消
    setMigrationMessage('正在取消迁移，请稍候...');
    try {
      await invoke('cancel_migration');
    } catch (error) {
      logger.error('取消迁移失败:', error);
    }
  }

  // 关闭迁移弹窗（成功/错误后的关闭）
  function handleCloseMigrationModal() {
    setMigrationModalOpen(false);
    setMigratingApp(null);
    setMigrationStep('idle');
    setMigrationMessage('');
    setMigrationProgress(0);
    setLockedProcesses([]);
  }

  // 迁移进行中点击 X → 二次确认后取消迁移并关闭弹窗
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

    // 发送取消信号给后端
    try {
      await invoke('cancel_migration');
    } catch (error) {
      logger.error('取消迁移失败:', error);
    }
    // 直接关闭弹窗，后端会自动回滚已复制内容
    handleCloseMigrationModal();
  }

  // 批量选择处理
  function handleToggleSelect(app: InstalledApp) {
    const key = app.registry_path || app.install_location;
    setSelectedKeys((prev) => {
      const next = new Set(prev);
      if (next.has(key)) {
        next.delete(key);
      } else {
        next.add(key);
      }
      return next;
    });
  }

  // 停止批量迁移：设置取消标志并通知后端停止当前任务
  function handleStopBatchMigrate() {
    batchCancelledRef.current = true;
    invoke('cancel_migration').catch(() => {});
  }

  function handleSelectAll() {
    const selectable = apps.filter((a) => !migratedPaths.some(
      (p) => p.toLowerCase() === a.install_location.toLowerCase()
    ));
    setSelectedKeys((prev) => {
      if (prev.size === selectable.length) {
        return new Set();
      }
      return new Set(selectable.map((a) => a.registry_path || a.install_location));
    });
  }

  // 批量迁移：依次迁移每个选中的应用
  async function handleBatchMigrate() {
    if (selectedKeys.size === 0) return;

    // 解析迁移目录（默认设置 / 引导设置 / 手动选择）
    const defaultTarget = loadAppDefaultTarget();
    const targetDir = await resolveMigrationTarget(defaultTarget, '批量迁移', setActiveTab, showTargetPicker);
    if (!targetDir) return;

    const selectedApps = apps.filter((a) =>
      selectedKeys.has(a.registry_path || a.install_location)
    );
    if (selectedApps.length === 0) return;

    const confirmed = await confirm(
      `即将批量迁移 ${selectedApps.length} 个应用到：\n${targetDir}\n\n每个应用将迁移到独立的子目录中，是否继续？`,
      { title: '确认批量迁移', kind: 'warning', okLabel: '开始迁移', cancelLabel: '取消' }
    );
    if (!confirmed) return;

    setBatchMigrating(true);
    batchCancelledRef.current = false;
    setBatchProgress({ current: 0, total: selectedApps.length });
    setSelectedKeys(new Set());

    let successCount = 0;
    let failCount = 0;

    for (let i = 0; i < selectedApps.length; i++) {
      // 用户手动停止批量迁移
      if (batchCancelledRef.current) {
        showToast(`批量迁移已停止，已完成 ${successCount} 个`, 'info');
        break;
      }

      const app = selectedApps[i];
      setBatchProgress({ current: i + 1, total: selectedApps.length });

      try {
        // 检查进程锁
        const lockResult = await invoke<ProcessLockResult>('check_process_locks', {
          sourcePath: app.install_location,
        });
        if (lockResult.is_locked) {
          showToast(`${app.display_name}: 文件被占用，跳过`, 'error');
          failCount++;
          continue;
        }

        let result = await invoke<MigrationResult>('migrate_app', {
          appName: app.display_name,
          source: app.install_location,
          targetParent: targetDir,
        });

        // 目标路径已有残留目录 → 弹出确认框，用户确认后以 force_overwrite 重试
        if (!result.success && (result.message.startsWith('TARGET_EXISTS_RETRY:') || result.message.startsWith('TARGET_EXISTS:'))) {
          if (batchCancelledRef.current) { failCount++; continue; }
          const isRetry = result.message.startsWith('TARGET_EXISTS_RETRY:');
          const existingPath = isRetry
            ? result.message.replace('TARGET_EXISTS_RETRY:', '')
            : result.message.replace('TARGET_EXISTS:', '');
          const promptMsg = isRetry
            ? `${app.display_name} 上次迁移未完全完成，目标位置存在残留目录：\n${existingPath}\n\n覆盖将清理残留并重新迁移。`
            : `${app.display_name} 的目标路径已存在残留目录：\n${existingPath}\n\n覆盖将删除该目录后重新迁移，是否继续？`;
          const overwrite = await confirm(
            promptMsg,
            { title: '目标目录已存在', kind: 'warning', okLabel: '覆盖并迁移', cancelLabel: '跳过' }
          );
          if (overwrite) {
            result = await invoke<MigrationResult>('migrate_app', {
              appName: app.display_name,
              source: app.install_location,
              targetParent: targetDir,
              forceOverwrite: true,
            });
          } else {
            showToast(`${app.display_name}: 已跳过（目标路径已存在残留目录）`, 'info');
            failCount++;
            continue;
          }
        }

        // 源路径是 Junction 且指向目标：恢复失败残留，覆盖迁移会丢失数据
        if (!result.success && result.message.startsWith('JUNCTION_LOOP:')) {
          showToast(`${app.display_name}: 原路径仍是链接，请先在迁移记录中恢复后重试`, 'error');
          failCount++;
          continue;
        }

        if (result.success) {
          successCount++;
        } else {
          showToast(`${app.display_name}: ${result.message}`, 'error');
          failCount++;
        }
      } catch (error) {
        showToast(`${app.display_name}: ${error}`, 'error');
        failCount++;
      }
    }

    setBatchMigrating(false);
    setBatchProgress({ current: 0, total: 0 });

    if (!batchCancelledRef.current) {
      if (failCount === 0) {
        showToast(`批量迁移完成：${successCount} 个全部成功`, 'success');
      } else {
        showToast(`批量迁移完成：${successCount} 成功, ${failCount} 失败`, 'info');
      }
    }
    await handleRefresh();
  }

  useEffect(() => {
    fetchInstalledApps();
    fetchAppMigrationRecords();
  }, []);

  return (
    <div className="h-full overflow-hidden flex flex-col" style={{ padding: 'var(--spacing-4) var(--spacing-5)' }}>
      <div className="flex-1 max-w-5xl mx-auto w-full min-h-0 flex flex-col overflow-hidden">
        <AppList
          apps={apps}
          loading={appsLoading}
          onMigrate={handleMigrate}
          onRestore={handleRestore}
            onUninstall={handleUninstall}
            onOpenFolder={handleOpenFolder}
            uninstallingKey={uninstallingKey}
            restoringKey={restoringKey}
            migratedPaths={migratedPaths}
            selectedKeys={selectedKeys}
            onToggleSelect={handleToggleSelect}
            onSelectAll={handleSelectAll}
            onBatchMigrate={handleBatchMigrate}
            onStopBatchMigrate={handleStopBatchMigrate}
            batchMigrating={batchMigrating}
            batchProgress={batchProgress}
            sizesLoading={sizesLoading}
            sizeMap={sizeMap}
            onRefresh={handleRefreshApps}
            refreshing={refreshing}
          />
      </div>

      {/* 迁移进度弹窗 */}
      <MigrationModal
        isOpen={migrationModalOpen}
        step={migrationStep}
        appName={migratingApp?.display_name || ''}
        message={migrationMessage}
        lockedProcesses={lockedProcesses}
        progress={migrationProgress}
        onCancel={handleCancelMigration}
        onForceContinue={handleForceContinue}
        onClose={handleCloseMigrationModal}
        onRequestClose={handleRequestCloseDuringMigration}
      />

      {/* 强力卸载残留清理弹窗 */}
      <CleanupModal
        isOpen={cleanupModalOpen}
        appName={cleanupTargetAppName}
        items={leftoverItems}
        loading={cleanupLoading}
        scanning={scanningResidue}
        onClose={handleCloseCleanupModal}
        onToggleItem={handleToggleLeftover}
        onConfirm={handleConfirmCleanup}
      />

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

      {/* Toast 通知 */}
      <Toast
        message={toast.message}
        type={toast.type}
        visible={toast.visible}
        onClose={hideToast}
      />
    </div>
  );
}
