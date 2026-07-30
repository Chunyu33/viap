// 应用迁移页面
// 实现完整的迁移流程：目录选择 -> 进程检测 -> 文件复制 -> 创建链接
// 支持真实进度上报和取消操作

import { useEffect, useState, useTransition, useCallback, useContext, useRef } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import { confirm, open } from '@tauri-apps/plugin-dialog';
import { Activity } from 'lucide-react';
import AppList from '../components/AppList';
import MigrationModal from '../components/MigrationModal';
import CleanupModal from '../components/CleanupModal';
import TargetPickerDialog from '../components/TargetPickerDialog';
import Toast, { useToast } from '../components/Toast';
import { logger } from '../utils/logger';
import { formatMigrationFailureMessage, parseTargetExistsConflict } from '../utils/migrationMessages';
import { TabNavigationContext } from '../App';
import { useDangerousPathCheck, WarningInfo } from '../hooks/useDangerousPathCheck';
import WarningConfirmDialog from '../components/WarningConfirmDialog';
import { useViapStore } from '../store';
import { readLocalUserSettings } from '../utils/userSettings';
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

// 流式扫描事件 payload 类型（与后端 ScanProgressEvent 一一对应）
interface IconUpdate {
  install_location: string;
  icon_base64: string;
  icon_url: string;
}

interface SizeUpdate {
  install_location: string;
  /** 目录大小，单位 KB */
  size_kb: number;
}

interface ScanProgressEvent {
  phase: 'snapshot' | 'tier1' | 'tier2' | 'tier3' | 'icons' | 'sizes' | 'sizes_done' | 'done';
  apps: InstalledApp[];           // 仅 tier1/tier2/tier3/done(缓存命中) 时有值
  icon_updates: IconUpdate[];     // 仅 icons 时有值
  size_updates: SizeUpdate[];     // 仅 sizes 时有值，由后台线程推送
  total_count: number;
  is_final: boolean;
}

interface ScanPerformanceEvent {
  /** 后端阶段名，保持原样展示便于对照日志 */
  phase: string;
  /** 从本次扫描开始到该阶段完成的累计耗时 */
  elapsed_ms: number;
  /** 该阶段累计应用数量 */
  total_count: number;
}

interface ScanNoticeEvent {
  code: string;
  message: string;
}

interface DebugMetric {
  /** 前端调试面板展示的阶段名 */
  phase: string;
  /** 阶段累计耗时，单位毫秒 */
  elapsedMs: number;
  /** 阶段累计应用数量 */
  totalCount: number;
}

// debug 校验状态只在设置开关开启时展示，避免普通用户看到开发指标。
type DebugUpdateState = 'idle' | 'checking' | 'updated' | 'synced';

interface FetchInstalledAppsOptions {
  /** 是否先推送持久化快照；手动刷新需要跳过快照，避免旧数据误导本次耗时 */
  useSnapshot?: boolean;
  /** 是否强制后端内存缓存失效；刷新和后台校验都需要拿到真实扫描结果 */
  forceRefresh?: boolean;
}

const SCAN_PHASE_LABELS: Record<string, string> = {
  snapshot: 'snapshot',
  tier1: 'tier1',
  tier2: 'tier2',
  tier3: 'tier3',
  tier3_skipped: 'tier3_skipped',
  done: 'done',
  icons_done: 'icons_done',
  sizes_done: 'sizes_done',
};

function loadShowScanDebug(): boolean {
  // 调试浮层默认关闭，设置统一从用户设置缓存读取。
  return readLocalUserSettings().showScanDebug;
}

/** 使用注册表路径优先做身份键，避免扫描刷新时同一应用图标被误覆盖 */
function appIdentityKey(app: InstalledApp): string {
  return (app.registry_path || app.install_location).toLowerCase();
}

/** 用稳定字段生成列表指纹，避免应用数量相同但缓存内容变化时误判为一致 */
function createAppsFingerprint(apps: InstalledApp[]): string {
  return apps
    .map(app => [
      appIdentityKey(app),
      app.display_name,
      app.install_location,
      app.display_icon,
      app.publisher,
    ].join('|').toLowerCase())
    .sort()
    .join('\n');
}

/** 合并后台扫描结果时保留已加载图标，避免快照/图标懒加载被空字段覆盖 */
function mergeAppsPreservingIcons(nextApps: InstalledApp[], previousApps: InstalledApp[]): InstalledApp[] {
  const previousByKey = new Map(previousApps.map(app => [appIdentityKey(app), app]));
  return nextApps.map(app => {
    const previous = previousByKey.get(appIdentityKey(app));
    if (!previous) return app;
    // 后端扫描结果可能先于图标懒加载事件返回，保留已可用的图标源避免闪回占位。
    return {
      ...app,
      icon_base64: app.icon_base64 || previous.icon_base64,
      icon_url: app.icon_url || previous.icon_url,
    };
  });
}

// Zustand store API 引用 — 组件外（回调/handler）通过 getState() 读写
const storeApi = useViapStore;

/** 从 localStorage 读取默认应用迁移目录，仅非 C 盘路径有效 */
function loadAppDefaultTarget(): string | null {
  const path = readLocalUserSettings().defaultAppTargetPath;
  // C 盘路径视为无效，需由用户重新选择。
  if (!path || path.startsWith('C:') || path.startsWith('c:')) return null;
  return path;
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

export default function AppMigration({ visible }: { visible: boolean }) {
  const [apps, setApps] = useState<InstalledApp[]>([]);
  const [appsLoading, setAppsLoading] = useState(true);
  const [sizesLoading, setSizesLoading] = useState(false);
  // 流式扫描阶段状态，用于展示进度提示
  const [scanPhase, setScanPhase] = useState<'idle' | 'snapshot' | 'tier1' | 'tier2' | 'tier3' | 'icons' | 'sizes' | 'sizes_done' | 'done'>('idle');
  const [scanTotalCount, setScanTotalCount] = useState(0);
  const [sizeMap, setSizeMap] = useState<Map<string, number>>(() => new Map(storeApi.getState().sizeMap));
  const [refreshing, setRefreshing] = useState(false);
  // 扫描耗时用于用户反馈时定位慢阶段，默认由设置页开关控制。
  const [showScanDebug, setShowScanDebug] = useState(() => loadShowScanDebug());
  const [debugMetrics, setDebugMetrics] = useState<DebugMetric[]>([]);
  const [debugCollapsed, setDebugCollapsed] = useState(false);
  const [debugUpdateState, setDebugUpdateState] = useState<DebugUpdateState>('idle');
  const [debugUpdateMessage, setDebugUpdateMessage] = useState('');
  const showScanDebugRef = useRef(showScanDebug);

  // 将应用列表相关的状态更新标记为低优先级，避免阻塞用户交互
  const [, startTransition] = useTransition();

  // 已迁移的路径列表
  const [migratedPaths, setMigratedPaths] = useState<string[]>([]);
  // Viap 自身的安装目录，用于禁用自身的迁移/卸载按钮
  const [viapInstallPath, setViapInstallPath] = useState<string>('');
  // 应用迁移记录（用于还原时获取 historyId）
  const [appMigrationRecords, setAppMigrationRecords] = useState<MigrationRecord[]>([]);

  // 迁移状态
  const [migrationModalOpen, setMigrationModalOpen] = useState(false);
  const [migrationStep, setMigrationStep] = useState<MigrationStep>('idle');
  const [migratingApp, setMigratingApp] = useState<InstalledApp | null>(null);
  const [migrationMessage, setMigrationMessage] = useState('');
  const [migrationProgress, setMigrationProgress] = useState(0);
  const [lockedProcesses, setLockedProcesses] = useState<string[]>([]);
  // 强力卸载状态
  const [uninstallingKey, setUninstallingKey] = useState<string | null>(null);
  // 还原状态
  const [restoringKey, setRestoringKey] = useState<string | null>(null);
  const [restoreProgressMap, setRestoreProgressMap] = useState<Record<string, number>>({});
  // 批量迁移
  const [selectedKeys, setSelectedKeys] = useState<Set<string>>(new Set());
  const [batchMigrating, setBatchMigrating] = useState(false);
  const [batchProgress, setBatchProgress] = useState({ current: 0, total: 0 });
  // 批量迁移取消标志，用 ref 避免异步循环中闭包捕获过期 state
  const batchCancelledRef = useRef(false);
  // 批量模式下进程锁弹窗的 Promise resolve 函数
  const batchProcessLockResolveRef = useRef<((value: boolean) => void) | null>(null);
  // 流式扫描事件监听器清理函数，用于组件卸载时取消监听
  const scanUnlistenRef = useRef<(() => void) | null>(null);
  const [batchWaitingProcessLock, setBatchWaitingProcessLock] = useState(false);
  const [cleanupModalOpen, setCleanupModalOpen] = useState(false);
  const [cleanupTargetAppName, setCleanupTargetAppName] = useState('');
  const [cleanupTargetPublisher, setCleanupTargetPublisher] = useState<string | null>(null);
  const [leftoverItems, setLeftoverItems] = useState<LeftoverItem[]>([]);
  const [cleanupLoading, setCleanupLoading] = useState(false);
  const [scanningResidue, setScanningResidue] = useState(false);

  // 强制删除预览弹窗状态 — 列出安装目录内容供用户确认后再删
  const [forceRmPreviewOpen, setForceRmPreviewOpen] = useState(false);
  const [forceRmPreviewApp, setForceRmPreviewApp] = useState<InstalledApp | null>(null);
  const [forceRmPreviewItems, setForceRmPreviewItems] = useState<LeftoverItem[]>([]);
  // 强制删除确认阶段保留用户选择的删除方式，避免预览后丢失设置。
  const [forceRmUseRecycleBin, setForceRmUseRecycleBin] = useState(true);
  const [forceRmLoading, setForceRmLoading] = useState(false);

  // Toast 通知
  const { toast, showToast, hideToast } = useToast();

  const { checkBlocked, checkWarning } = useDangerousPathCheck();

  useEffect(() => {
    showScanDebugRef.current = showScanDebug;
  }, [showScanDebug]);

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

  // 流式扫描：初次进入页面时使用，通过 scan-progress 事件分阶段推送
  // Tier 1 完成后立即显示首批应用（~200ms），图标和大小在后台静默填入
  // 若 appStore 已缓存（Tab 切换后再回来），直接恢复 state，零 IPC 开销
  async function fetchInstalledApps(options: FetchInstalledAppsOptions = {}) {
    setShowScanDebug(loadShowScanDebug());
    // ── 缓存命中 ──
    if (!options.forceRefresh && storeApi.getState().isScanned) {
      startTransition(() => setApps([...storeApi.getState().apps]));
      setScanPhase('done');
      setScanTotalCount(storeApi.getState().apps.length);
      setAppsLoading(false);
      if (storeApi.getState().isSizesLoaded) {
        startTransition(() => setSizeMap(new Map(storeApi.getState().sizeMap)));
      } else {
        // 后台线程仍在计算大小，重新监听 sizes/sizes_done 事件
        setSizesLoading(true);
        const sizeAccumulator = new Map<string, number>();
        const pending = new Map<string, number>();
        let timer: ReturnType<typeof setTimeout> | null = null;

        const sizeUnlisten = await listen<ScanProgressEvent>('scan-progress', (event) => {
          const { phase, size_updates } = event.payload;
          if (phase === 'sizes' && size_updates.length > 0) {
            for (const u of size_updates) {
              pending.set(u.install_location.toLowerCase(), u.size_kb);
            }
            if (!timer) {
              timer = setTimeout(() => {
                if (pending.size === 0) return;
                timer = null;
                for (const [k, v] of pending) { sizeAccumulator.set(k, v); }
                pending.clear();
                startTransition(() => setSizeMap(new Map(sizeAccumulator)));
              }, 300);
            }
          }
          if (phase === 'sizes_done') {
            if (timer) { clearTimeout(timer); timer = null; }
            // flush pending
            for (const [k, v] of pending) { sizeAccumulator.set(k, v); }
            pending.clear();
            storeApi.setState({ sizeMap: new Map(sizeAccumulator) });
            storeApi.setState({ isSizesLoaded: true });

            startTransition(() => setSizeMap(new Map(sizeAccumulator)));
            setSizesLoading(false);
            sizeUnlisten();
          }
        });
        // 组件卸载或重新扫描时会通过 scanUnlistenRef 清理
        scanUnlistenRef.current = () => { if (timer) clearTimeout(timer); sizeUnlisten(); };
      }
      return;
    }

    // ── 首次扫描 ──
    setAppsLoading(true);
    setScanPhase('idle');
    setDebugMetrics([]);
    setDebugUpdateState('idle');
    setDebugUpdateMessage('');

    // appMap：扫描期间的临时缓存，仅用于 tier1 预览阶段
    const appMap = new Map<string, InstalledApp>();
    // 有快照时保持完整旧列表，避免 Tier1/Tier2 的增量结果让列表短暂变少。
    let hasSnapshotPreview = false;

    // 图标缓冲：后端仅推送 icon_url，真实 PNG 由 WebView 通过自定义协议懒加载。
    const pendingIcons = new Map<string, string>();
    let iconTimer: ReturnType<typeof setTimeout> | null = null;

    function flushIcons() {
      if (pendingIcons.size === 0) return;
      if (iconTimer) { clearTimeout(iconTimer); iconTimer = null; }
      const snap = new Map(pendingIcons);
      pendingIcons.clear();
      startTransition(() => {
        setApps(prev => {
          let changed = false;
          const next = prev.map(app => {
            const iconSource = snap.get(app.install_location.toLowerCase());
            if (iconSource?.startsWith('http://viap-icon.localhost/') && iconSource !== app.icon_url) {
              changed = true;
              return { ...app, icon_url: iconSource };
            }
            if (iconSource && iconSource !== app.icon_base64) {
              changed = true;
              return { ...app, icon_base64: iconSource };
            }
            return app;
          });
          // 同步回写 appStore，防止 Tab 切换后图标丢失
          // flushIcons 只更新了 React state，storeApi.getState().apps 仍持有时刻全空的对象
          if (changed) { storeApi.setState({ apps: next }); }
          return changed ? next : prev;
        });
      });
    }

    // 大小缓冲：后台线程通过 sizes 事件分批推送目录大小
    // 节流 300ms 合并多批后刷新 UI，避免频繁 re-render
    const accumulatedSizes = new Map<string, number>();
    const pendingSizes = new Map<string, number>();
    let sizesTimer: ReturnType<typeof setTimeout> | null = null;

    function flushSizes() {
      if (pendingSizes.size === 0) return;
      if (sizesTimer) { clearTimeout(sizesTimer); sizesTimer = null; }
      for (const [key, size] of pendingSizes) {
        accumulatedSizes.set(key, size);
      }
      pendingSizes.clear();
      startTransition(() => setSizeMap(new Map(accumulatedSizes)));
    }

    const performanceUnlisten = await listen<ScanPerformanceEvent>('scan-performance', (event) => {
      const metric = event.payload;
      setDebugMetrics(prev => [
        ...prev.filter(item => item.phase !== metric.phase),
        {
          phase: metric.phase,
          elapsedMs: metric.elapsed_ms,
          totalCount: metric.total_count,
        },
      ]);
    });

    const noticeUnlisten = await listen<ScanNoticeEvent>('scan-notice', (event) => {
      // 后端只在启动阶段延后深度扫描时发送提示，保留较长时间方便用户读完并执行手动刷新。
      if (event.payload.message.trim()) {
        showToast(event.payload.message, 'info', 12000);
      }
    });

    const unlisten = await listen<ScanProgressEvent>('scan-progress', (event) => {
      const { phase, apps: newApps, icon_updates, total_count, is_final } = event.payload;

      // 图标和大小阶段不覆写 scanPhase，避免进度提示条闪烁
      if (phase !== 'icons' && phase !== 'sizes' && phase !== 'sizes_done') {
        setScanPhase(phase);
        setScanTotalCount(total_count);
      }

      if (phase === 'snapshot') {
        hasSnapshotPreview = true;
        if (showScanDebugRef.current) {
          setDebugUpdateState('checking');
          setDebugUpdateMessage('已显示缓存快照，正在后台校验应用列表...');
        }
        const sorted = [...newApps].sort((a, b) =>
          a.display_name.toLowerCase().localeCompare(b.display_name.toLowerCase())
        );
        storeApi.setState({ apps: sorted });
        storeApi.setState({ scanPhase: 'snapshot' });
        storeApi.setState({ scanTotalCount: sorted.length });
        startTransition(() => setApps(sorted));
        setScanTotalCount(sorted.length);
        setAppsLoading(false);
      }

      if (phase === 'tier1' || phase === 'tier2' || phase === 'tier3') {
        for (const app of newApps) {
          appMap.set(app.install_location.toLowerCase(), app);
        }
        if (!hasSnapshotPreview && (phase === 'tier1' || apps.length === 0) && newApps.length > 0) {
          const sorted = [...appMap.values()].sort((a, b) =>
            a.display_name.toLowerCase().localeCompare(b.display_name.toLowerCase())
          );
          startTransition(() => setApps(sorted));
          setAppsLoading(false);
        }
      }

      if (phase === 'icons' && icon_updates.length > 0) {
        for (const u of icon_updates) {
          pendingIcons.set(u.install_location.toLowerCase(), u.icon_url || u.icon_base64);
        }
        if (iconTimer) clearTimeout(iconTimer);
        iconTimer = setTimeout(flushIcons, 200);
      }

      // sizes 事件由后台线程推送，节流合并后刷新 UI
      if (phase === 'sizes' && event.payload.size_updates.length > 0) {
        for (const u of event.payload.size_updates) {
          pendingSizes.set(u.install_location.toLowerCase(), u.size_kb);
        }
        if (!sizesTimer) {
          sizesTimer = setTimeout(flushSizes, 300);
        }
      }

      // sizes_done：后台线程全部完成，最终写入 appStore + 模块级缓存
      if (phase === 'sizes_done') {
        if (sizesTimer) { clearTimeout(sizesTimer); sizesTimer = null; }
        flushSizes();
        storeApi.setState({ sizeMap: new Map(accumulatedSizes) });
        storeApi.setState({ isSizesLoaded: true });
        setSizesLoading(false);
        setScanPhase('done');
        // 后台线程结束，清理 scan-progress 监听器
        scanUnlistenRef.current?.();
        scanUnlistenRef.current = null;
      }

      if (is_final) {
        setAppsLoading(false);
        setScanPhase('done');
        // done 事件之后后台线程开始计算大小，启动 loading 指示
        setSizesLoading(true);
      }
    });

    scanUnlistenRef.current = () => {
      unlisten();
      performanceUnlisten();
      noticeUnlisten();
    };

    try {
      // fullResult 是去重+failsafe+图标 URL 的完整结果，保留已加载图标避免覆盖闪烁。
      const fullResult = await invoke<InstalledApp[]>('get_installed_apps_stream', {
        useSnapshot: options.useSnapshot ?? true,
        forceRefresh: options.forceRefresh ?? false,
      });

      // 写入 appStore（不清理 listener —— 后台大小线程仍在推送 sizes/sizes_done 事件）
      if (iconTimer) clearTimeout(iconTimer);

      const previousApps = storeApi.getState().apps.length > 0 ? storeApi.getState().apps : apps;
      const previousFingerprint = createAppsFingerprint(previousApps);
      const mergedResult = mergeAppsPreservingIcons(fullResult, previousApps);
      const nextFingerprint = createAppsFingerprint(mergedResult);
      if (showScanDebugRef.current && previousApps.length > 0) {
        if (previousFingerprint !== nextFingerprint) {
          setDebugUpdateState('updated');
          setDebugUpdateMessage(`后台校验完成，列表已更新：${previousApps.length} → ${mergedResult.length}`);
        } else {
          setDebugUpdateState('synced');
          setDebugUpdateMessage('后台校验完成，缓存与当前扫描一致');
        }
      }
      storeApi.setState({ apps: mergedResult });
      storeApi.setState({ isScanned: true });
      storeApi.setState({ scanPhase: 'done' });
      storeApi.setState({ scanTotalCount: mergedResult.length });
      startTransition(() => setApps(mergedResult));
      setScanPhase('done');
      setAppsLoading(false);
      // 大小由后台线程通过 sizes 事件推送，sizes_done 到来时清理 listener
    } catch (error) {
      logger.error('流式扫描失败:', error);
      scanUnlistenRef.current?.();
      scanUnlistenRef.current = null;
      try {
        const fallbackApps = await invoke<InstalledApp[]>('get_installed_apps');
        storeApi.setState({ apps: fallbackApps });
        storeApi.setState({ isScanned: true });
        storeApi.setState({ scanPhase: 'done' });
        storeApi.setState({ scanTotalCount: fallbackApps.length });
        startTransition(() => setApps(fallbackApps));
        // 降级路径：fallback 走的是 scan_all 同步路径，estimated_size 已填入
        // 直接构建 sizeMap 避免又走 loadAppSizes（已被删除）
        const fallbackSizeMap = new Map<string, number>();
        for (const a of fallbackApps) {
          if (a.estimated_size > 0) {
            fallbackSizeMap.set(a.registry_path || a.install_location, a.estimated_size);
          }
        }
        storeApi.setState({ sizeMap: fallbackSizeMap });
        storeApi.setState({ isSizesLoaded: true });

        startTransition(() => setSizeMap(fallbackSizeMap));
      } catch {
        startTransition(() => setApps([]));
      }
      setAppsLoading(false);
      setScanPhase('done');
    }
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
    // 先终止所有进行中的扫描监听（图标后台线程的 emit 将无人接收）
    scanUnlistenRef.current?.();
    scanUnlistenRef.current = null;

    // 清空 appStore 缓存
    storeApi.setState({ isScanned: false });
    storeApi.setState({ isSizesLoaded: false });
    storeApi.setState({ sizeMap: new Map() });
    storeApi.setState({ apps: [] });


    try {
      // refresh_apps 调用后端 scan_all（同步，含图标，不走流式）
      const freshApps = await invoke<InstalledApp[]>('refresh_apps');

      storeApi.setState({ apps: freshApps });
      storeApi.setState({ isScanned: true });
      storeApi.setState({ scanPhase: 'done' });
      storeApi.setState({ scanTotalCount: freshApps.length });

      startTransition(() => setApps(freshApps));
      setScanPhase('done');
      setScanTotalCount(freshApps.length);
      setAppsLoading(false);

      // refresh_apps 内部调用 scan_all，已完成并行大小计算
      // estimated_size 已填入（KB），直接构建 sizeMap
      const refreshSizeMap = new Map<string, number>();
      for (const a of freshApps) {
        if (a.estimated_size > 0) {
          refreshSizeMap.set(a.registry_path || a.install_location, a.estimated_size);
        }
      }
      storeApi.setState({ sizeMap: refreshSizeMap });
      storeApi.setState({ isSizesLoaded: true });

      startTransition(() => setSizeMap(refreshSizeMap));
    } catch (error) {
      logger.error('刷新应用列表失败:', error);
    }
    await fetchAppMigrationRecords();
  }

  // 手动刷新：终止监听 + 清空 appStore + 后端强刷全量扫描
  async function handleRefreshApps() {
    setRefreshing(true);

    // 先终止所有进行中的扫描监听
    scanUnlistenRef.current?.();
    scanUnlistenRef.current = null;

    storeApi.setState({ isScanned: false });
    storeApi.setState({ isSizesLoaded: false });
    storeApi.setState({ sizeMap: new Map() });
    storeApi.setState({ apps: [] });
    setApps([]);
    setSizeMap(new Map());
    setAppsLoading(true);
    setSizesLoading(false);
    setScanPhase('idle');
    setScanTotalCount(0);
    setDebugMetrics([]);
    setDebugUpdateState('idle');
    setDebugUpdateMessage('');

    try {
      // 刷新复用流式扫描，这样 debug 耗时、图标 URL、大小后台缓存都会随本次刷新更新。
      await fetchInstalledApps({ useSnapshot: false, forceRefresh: true });
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
    let unlisten: UnlistenFn | null = null;

    try {
      setRestoringKey(currentRestoreKey);
      setRestoreProgressMap(prev => ({ ...prev, [currentRestoreKey]: 0 }));

      try {
        unlisten = await listen<MigrationProgressEvent>('migration-progress', (event) => {
          const data = event.payload;
          // 恢复事件以原始路径作为 task_id，只更新当前行，避免迁移页其他任务串进来。
          if (data.task_id.toLowerCase() === record.original_path.toLowerCase()) {
            setRestoreProgressMap(prev => ({ ...prev, [currentRestoreKey]: data.percent }));
          }
        });
      } catch (error) {
        logger.error('注册恢复进度监听失败:', error);
      }

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
      setRestoreProgressMap(prev => {
        const next = { ...prev };
        delete next[currentRestoreKey];
        return next;
      });
      setRestoringKey(null);
      // 恢复完成或失败后清理监听，避免重复点击时累积 listener。
      if (unlisten) unlisten();
    }
  }

  // 强制删除预览弹窗 — 切换勾选
  function handleForceRmToggleItem(path: string) {
    setForceRmPreviewItems(prev =>
      prev.map(item => item.path === path ? { ...item, selected: !item.selected } : item)
    );
  }

  // 强制删除预览弹窗 — 确认删除
  async function handleForceRmConfirm() {
    const app = forceRmPreviewApp;
    if (!app) return;
    const currentUninstallKey = `${app.display_name}|${app.registry_path}`;
    try {
      setForceRmLoading(true);
      setUninstallingKey(currentUninstallKey);
      const result = await invoke<UninstallResult>('force_remove_application', {
        input: {
          app_id: app.display_name,
          registry_path: app.registry_path,
          install_location: app.install_location,
          display_icon: app.display_icon,
          use_recycle_bin: forceRmUseRecycleBin,
          selected_paths: forceRmPreviewItems.filter(item => item.selected).map(item => item.path),
        },
      });
      if (result.success) {
        setForceRmPreviewOpen(false);
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
      setForceRmLoading(false);
      setUninstallingKey(null);
    }
  }

  /** 强制删除入口：先预览文件列表 → 用户确认 → 执行删除 */
  async function forceRemoveApp(app: InstalledApp, useRecycleBin: boolean): Promise<boolean> {
    try {
      // 第一步：预览安装目录内容
      const items = await invoke<LeftoverItem[]>('preview_force_remove', {
        input: {
          app_id: app.display_name,
          install_location: app.install_location,
          display_icon: app.display_icon,
        },
      });
      if (items.length === 0) {
        // 目录为空，直接尝试删除
        const result = await invoke<UninstallResult>('force_remove_application', {
          input: {
            app_id: app.display_name,
            registry_path: app.registry_path,
            install_location: app.install_location,
            display_icon: app.display_icon,
            use_recycle_bin: useRecycleBin,
            selected_paths: null,
          },
        });
        if (result.success) {
          showToast(`${app.display_name} 已删除（目录为空）`, 'success');
          await handleRefresh();
        } else {
          showToast(result.message || '删除失败', 'error');
        }
        return false;
      }
      // 第二步：打开预览弹窗等待用户确认
      setForceRmUseRecycleBin(useRecycleBin);
      setForceRmPreviewApp(app);
      setForceRmPreviewItems(items);
      setForceRmPreviewOpen(true);
      // 预览弹窗仍属于卸载流程，直到用户取消或确认删除前都保持全局操作锁。
      return true;
    } catch (error) {
      showToast(`预览安装目录失败: ${error}`, 'error');
      return false;
    }
  }

  // 强力卸载流程
  async function handleUninstall(app: InstalledApp) {
    // 兜底防护：Viap 自身不可卸载
    if (isViapSelf(app)) {
      showToast('Viap 自身不可卸载', 'error');
      return;
    }

    const currentUninstallKey = `${app.display_name}|${app.registry_path}`;
    // 整个卸载流程复用同一份删除策略，异常转强制删除时也必须保持一致。
    const useRecycleBin = readLocalUserSettings().useRecycleBin;
    let keepUninstallLock = false;
    try {
      // 从卸载流程开始就锁定列表，避免预览确认期间用户又启动其他文件操作。
      setUninstallingKey(currentUninstallKey);

      // 先预览卸载命令
      let previewCommands: string[] = [];
      let previewFailed = false;
      try {
        const preview = await invoke<UninstallPreview>('preview_uninstall', {
          input: {
            app_id: app.display_name,
            registry_path: app.registry_path,
            install_location: app.install_location,
            display_icon: app.display_icon,
            use_recycle_bin: useRecycleBin,
          },
        });
        previewCommands = preview.commands;
      } catch {
        previewFailed = true;
      }

      // 卸载程序不可用（损坏/缺失）→ 走强制删除流程
      if (previewFailed || previewCommands.length === 0) {
        const forceConfirm = await confirm(
          `${app.display_name} 的卸载程序不可用（可能已损坏或被删除）。\n\n是否执行强制删除？将直接移除安装目录和注册表项。`,
          { title: '强制删除', kind: 'warning', okLabel: '强制删除', cancelLabel: '取消' }
        );
        if (!forceConfirm) return;
        keepUninstallLock = await forceRemoveApp(app, useRecycleBin);
        return;
      }

      // 正常卸载流程
      const commandLines = `\n\n即将执行的卸载命令：\n${previewCommands.map((c, i) => `  ${i + 1}. ${c}`).join('\n')}`;

      const confirmed = await confirm(
        `即将启动 ${app.display_name} 的卸载程序。\n\n此操作可能删除应用及其相关组件，是否继续？${commandLines}`,
        { title: '确认强力卸载', kind: 'warning', okLabel: '继续卸载', cancelLabel: '取消' }
      );
      if (!confirmed) return;

      const result = await invoke<UninstallResult>('uninstall_application', {
        input: {
          app_id: app.display_name,
          registry_path: app.registry_path,
          install_location: app.install_location,
          display_icon: app.display_icon,
          use_recycle_bin: useRecycleBin,
        },
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
          keepUninstallLock = await forceRemoveApp(app, useRecycleBin);
        }
      } else {
        showToast(`卸载未完成：${error}`, 'error');
      }
    } finally {
      if (!keepUninstallLock) {
        setUninstallingKey(null);
      }
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

  // Viap 自身不可迁移/卸载（兜底防护，UI 层已禁用按钮）
  function isViapSelf(app: InstalledApp): boolean {
    if (!viapInstallPath) return false;
    return app.install_location.toLowerCase().replace(/\//g, '\\') ===
      viapInstallPath.toLowerCase().replace(/\//g, '\\');
  }

  // 核心迁移流程
  async function handleMigrate(app: InstalledApp) {
    // 兜底防护：Viap 自身不可迁移
    if (isViapSelf(app)) {
      showToast('Viap 自身不可迁移', 'error');
      return;
    }

    // 步骤 0: BLOCKED 前端拦截（后端 migration.rs 也有兜底防线）
    const blockedMsg = checkBlocked(app.install_location);
    if (blockedMsg) {
      showToast(blockedMsg, 'error');
      return;
    }

    // 步骤 0.5: WARNING 检测 — 非批量模式弹确认弹窗
    let localConfirmedWarning = false;
    const warningInfo = checkWarning(app.install_location);
    if (warningInfo) {
      const confirmed = await showWarningDialog(warningInfo);
      if (!confirmed) return;
      localConfirmedWarning = true;
    }

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
        setLockedProcesses(lockResult.processes);
        return;
      }

      // 无进程占用，直接开始复制
      await startCopyPhase(app, targetDir as string, localConfirmedWarning);
    } catch (error) {
      setMigrationStep('error');
      setMigrationMessage(`检测进程锁失败: ${error}`);
    }
  }

  // 批量迁移进程锁：跳过当前应用，继续批量
  function handleBatchProcessLockSkip() {
    batchProcessLockResolveRef.current?.(true);
  }
  // 批量迁移进程锁：停止批量迁移
  function handleBatchProcessLockStop() {
    batchProcessLockResolveRef.current?.(false);
  }

  // 开始文件复制阶段（带事件监听）
  async function startCopyPhase(app: InstalledApp, targetDir: string, userConfirmedWarning: boolean) {
    setMigrationStep('checking');
    setLockedProcesses([]);

    // 注册进度事件监听器
    let unlisten: UnlistenFn | null = null;
    try {
      unlisten = await listen<MigrationProgressEvent>('migration-progress', (event) => {
        const data = event.payload;
        setMigrationProgress(data.percent);

        // 根据后端 step 同步前端步骤
        switch (data.step) {
          case 'checking':
            setMigrationStep('checking');
            break;
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
        userConfirmedWarning,
      });

      const targetConflict = parseTargetExistsConflict(result.message);
      // 目标路径已有残留目录 → 询问用户是否覆盖，避免内部状态码进入弹窗。
      if (!result.success && targetConflict) {
        const promptMsg = targetConflict.isRetry
          ? `目标位置已存在同名目录：\n${targetConflict.existingPath}\n\n该目录可能是上次迁移未完成留下的残留，也可能是手动创建的数据。\n覆盖将删除该目录后重新迁移，是否继续？`
          : `目标路径已存在同名目录：\n${targetConflict.existingPath}\n\n覆盖将删除该目录后重新迁移，是否继续？`;
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
          userConfirmedWarning,
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

      // 后端兜底：前端未传 userConfirmedWarning 时返回此错误码
      // 正常流程不会到这里（前端已弹窗确认），仅作为防绕过最后防线
      if (!result.success && result.message.startsWith('REQUIRES_WARNING_CONFIRM:')) {
        if (unlisten) unlisten();
        setMigrationStep('error');
        setMigrationMessage('迁移被拒绝：高风险目录需通过正常流程确认，请重新操作。');
        return;
      }

      // 后端步骤 0.5 检测到进程占用（比前端 check_process_locks 更准确，
      // 能检测到 Language Server、IntelliCode 等后台子进程）
      // 解析进程名列表，走与前端步骤 2 相同的进程占用提示 UI
      if (!result.success && result.message.includes('检测到以下程序正在运行')) {
        if (unlisten) unlisten();
        // 后端消息格式：'检测到以下程序正在运行，请关闭后重试：\nProc1.exe、Proc2.exe'
        const lines = result.message.split('\n');
        const processLine = lines.find(l => !l.includes('检测到以下程序') && l.trim().length > 0);
        const processes = processLine
          ? processLine.split('、').map(p => p.trim()).filter(Boolean)
          : ['（未知进程）'];
        setLockedProcesses(processes);
        setMigrationStep('checking');
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
        setMigrationMessage(formatMigrationFailureMessage(result.message));
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
    // 同时通知后端取消当前正在执行的 migrate_app（如果有），覆盖当前 invoke 的复制/扫描阶段
    invoke('cancel_migration').catch(() => {});
    // 列表刷新在 handleBatchMigrate 的 finally 段（handleRefresh）中处理，这里不额外刷新避免竞态
  }

  function handleSelectAll() {
    const selectable = apps.filter((a) =>
      !migratedPaths.some(
        (p) => p.toLowerCase() === a.install_location.toLowerCase()
      ) && !isViapSelf(a) // Viap 自身不可迁移，排除在批量操作之外
    );
    setSelectedKeys((prev) => {
      if (prev.size === selectable.length) {
        return new Set();
      }
      return new Set(selectable.map((a) => a.registry_path || a.install_location));
    });
  }

  // 批量迁移：依次迁移每个选中的应用，复用单个迁移的进度弹窗
  async function handleBatchMigrate() {
    if (selectedKeys.size === 0) return;

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

    // 初始化批量状态
    setBatchMigrating(true);
    batchCancelledRef.current = false;
    setBatchProgress({ current: 0, total: selectedApps.length });
    setSelectedKeys(new Set());

    // 打开进度弹窗（复用单个迁移的 MigrationModal）
    setMigrationModalOpen(true);
    setMigrationStep('counting');
    setMigrationMessage('准备开始批量迁移...');
    setMigrationProgress(0);
    setLockedProcesses([]);

    // 注册进度事件监听器（批量期间持续监听）
    let unlisten: UnlistenFn | null = null;
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
    } catch (error) {
      logger.error('注册批量进度监听失败:', error);
    }

    let successCount = 0;
    let failCount = 0;
    const failedApps: string[] = [];

    for (let i = 0; i < selectedApps.length; i++) {
      if (batchCancelledRef.current) break;

      const app = selectedApps[i];
      setBatchProgress({ current: i + 1, total: selectedApps.length });

      // 更新弹窗标题为当前正在迁移的应用名
      setMigratingApp(app);
      setMigrationStep('checking');
      setMigrationProgress(0);
      setMigrationMessage(`正在处理 (${i + 1}/${selectedApps.length})...`);

      try {
        // BLOCKED 前端拦截（后端 migration.rs 也有兜底防线）
        const blockedMsg = checkBlocked(app.install_location);
        if (blockedMsg) {
          showToast(`${app.display_name}: ${blockedMsg}`, 'error');
          failCount++;
          failedApps.push(app.display_name);
          continue;
        }

        // WARNING 检测 — 批量模式不弹窗，直接跳过提示单独迁移
        const warningInfo = checkWarning(app.install_location);
        if (warningInfo) {
          showToast(
            `${app.display_name}: 位于「${warningInfo.category}」(${warningInfo.label})，存在风险，已跳过。\n请在应用列表中单独迁移此项以查看详情。`,
            'info'
          );
          failCount++;
          failedApps.push(`${app.display_name}（高风���跳过）`);
          continue;
        }

        // 直接调用 migrate_app，后端步骤 0.5 会做准确的占用检测
        // 不在此处调用 check_process_locks（弱检测且后端已覆盖）
        let result = await invoke<MigrationResult>('migrate_app', {
          appName: app.display_name,
          source: app.install_location,
          targetParent: targetDir,
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
          setMigrationMessage(`${app.display_name} 有进程占用，请处理后选择继续或跳过`);

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
          showToast(`${app.display_name}: 存在进程占用，已跳过`, 'info');
          failCount++;
          failedApps.push(`${app.display_name}（进程占用跳过）`);
          continue;
        }

        // 目标路径已有残留目录 → 弹出确认框，用户确认后以 force_overwrite 重试
        const targetConflict = parseTargetExistsConflict(result.message);
        if (!result.success && targetConflict) {
          if (batchCancelledRef.current) { failCount++; failedApps.push(app.display_name); continue; }
          const promptMsg = targetConflict.isRetry
            ? `${app.display_name} 的目标位置已存在同名目录：\n${targetConflict.existingPath}\n\n该目录可能是上次迁移未完成留下的残留，也可能是手动创建的数据。\n覆盖将删除该目录后重新迁移，是否继续？`
            : `${app.display_name} 的目标路径已存在：\n${targetConflict.existingPath}\n\n覆盖将删除该目录后重新迁移，是否继续？`;
          const overwrite = await confirm(promptMsg, {
            title: '目标目录已存在',
            kind: 'warning',
            okLabel: '覆盖并迁移',
            cancelLabel: '跳过',
          });
          if (overwrite) {
            result = await invoke<MigrationResult>('migrate_app', {
              appName: app.display_name,
              source: app.install_location,
              targetParent: targetDir,
              forceOverwrite: true,
              userConfirmedWarning: false,
            });
          } else {
            showToast(`${app.display_name}: 已跳过（目标路径已存在）`, 'info');
            failCount++;
            failedApps.push(app.display_name);
            continue;
          }
        }

        // Junction 循环检测：原路径仍是 Junction 指向目标盘
        if (!result.success && result.message.startsWith('JUNCTION_LOOP:')) {
          showToast(`${app.display_name}: 原路径仍是链接，请先在迁移记录中恢复后重试`, 'error');
          failCount++;
          failedApps.push(app.display_name);
          continue;
        }

        if (result.success) {
          successCount++;
        } else {
          // 后端返回的错误（文件占用、空间不足、完整性校验失败等）
          showToast(`${app.display_name}: ${formatMigrationFailureMessage(result.message)}`, 'error');
          failCount++;
          failedApps.push(app.display_name);
        }
      } catch (error) {
        const errStr = String(error);
        if (!errStr.includes('用户取消了迁移')) {
          showToast(`${app.display_name}: ${error}`, 'error');
          failedApps.push(app.display_name);
        }
        failCount++;
      }
    }

    // 清理监听器
    if (unlisten) unlisten();

    setBatchMigrating(false);
    setBatchProgress({ current: 0, total: 0 });

    // 关闭进度弹窗，显示汇总结果
    handleCloseMigrationModal();

    if (batchCancelledRef.current) {
      showToast(`批量迁移已停止，已完成 ${successCount} 个`, 'info');
    } else if (failCount === 0) {
      showToast(`批量迁移完成：${successCount} 个全部成功`, 'success');
    } else {
      showToast(
        `批量迁移完成：${successCount} 成功，${failCount} 失败` +
        (failedApps.length > 0 ? `\n失败：${failedApps.join('、')}` : ''),
        'info'
      );
    }

    await handleRefresh();
  }

  useEffect(() => {
    // 仅在页面可见时触发：已有缓存 → 恢复本地状态；无缓存 → 开始流式扫描
    if (!visible) return;
    setShowScanDebug(loadShowScanDebug());
    if (storeApi.getState().isScanned) {
      // 从 Zustand 恢复本地状态（组件因 CSS 定位重建导致 state 丢失时兜底）
      const st = storeApi.getState();
      startTransition(() => {
        setApps([...st.apps]);
        setSizeMap(new Map(st.sizeMap));
      });
      setScanPhase('done');
      setScanTotalCount(st.apps.length);
      setAppsLoading(false);
      setSizesLoading(false);
      return;
    }
    fetchInstalledApps();
    fetchAppMigrationRecords();
    // 获取 Viap 自身安装目录，用于禁用自身的迁移/卸载按钮
    invoke<string>('get_viap_install_path')
      .then(setViapInstallPath)
      .catch(() => {});
    // 组件卸载时清理流式扫描事件监听器
    return () => {
      scanUnlistenRef.current?.();
    };
  }, [visible]);

  return (
    <div className="relative h-full overflow-hidden flex flex-col" style={{ padding: 'var(--spacing-4) var(--spacing-5)' }}>
      <div className="flex-1 max-w-5xl mx-auto w-full min-h-0 flex flex-col overflow-hidden">
        {/* debug 浮层使用绝对定位，折叠态只保留左侧小图标，避免抢占主体信息。 */}
        {showScanDebug && (debugMetrics.length > 0 || debugUpdateMessage) && (
          <div
            className="pointer-events-none absolute left-3 top-3 z-40 flex items-start gap-2 text-[11px]"
            style={{
              color: 'var(--text-secondary)',
            }}
          >
            <button
              type="button"
              onClick={() => setDebugCollapsed(prev => !prev)}
              className="pointer-events-auto flex h-6 w-6 items-center justify-center rounded-md transition-colors"
              style={{
                background: 'color-mix(in srgb, var(--bg-modal) 92%, transparent)',
                border: '1px solid var(--border-color)',
                color: debugUpdateState === 'updated' ? 'var(--color-success)' : 'var(--text-tertiary)',
                boxShadow: 'var(--shadow-sm)',
                backdropFilter: 'blur(8px)',
              }}
              title={debugCollapsed ? '展开扫描耗时' : '折叠扫描耗时'}
            >
              <Activity className="h-3.5 w-3.5" />
            </button>
            <div
              className="pointer-events-auto overflow-hidden transition-all duration-300 ease-out"
              style={{
                maxWidth: debugCollapsed ? 0 : '280px',
                opacity: debugCollapsed ? 0 : 1,
                transform: debugCollapsed ? 'translateX(-6px)' : 'translateX(0)',
              }}
            >
              <div
                className="rounded-md px-3 py-2"
                style={{
                  width: 280,
                  background: 'color-mix(in srgb, var(--bg-modal) 94%, transparent)',
                  border: '1px solid var(--border-color)',
                  boxShadow: 'var(--shadow-md)',
                  backdropFilter: 'blur(8px)',
                }}
              >
                <div className="mb-1 flex items-center justify-between gap-2">
                  <span className="font-medium" style={{ color: 'var(--text-primary)' }}>
                    Scan debug
                  </span>
                  {debugUpdateState !== 'idle' && (
                    <span
                      className="rounded px-1.5 py-0.5"
                      style={{
                        background: debugUpdateState === 'updated' ? 'var(--color-success-light)' : 'var(--bg-hover)',
                        color: debugUpdateState === 'updated' ? 'var(--color-success)' : 'var(--text-tertiary)',
                      }}
                    >
                      {debugUpdateState === 'checking' ? '校验中' : debugUpdateState === 'updated' ? '已更新' : '已同步'}
                    </span>
                  )}
                </div>
                <div className="grid grid-cols-2 gap-x-3 gap-y-1 tabular-nums" style={{ color: 'var(--text-secondary)' }}>
                  {debugMetrics.map(metric => (
                    <span key={metric.phase} className="truncate">
                      {SCAN_PHASE_LABELS[metric.phase] ?? metric.phase}: {metric.elapsedMs}ms
                    </span>
                  ))}
                </div>
              </div>
            </div>
          </div>
        )}
        <AppList
          apps={apps}
          loading={appsLoading}
          onMigrate={handleMigrate}
          onRestore={handleRestore}
            onUninstall={handleUninstall}
            onOpenFolder={handleOpenFolder}
            uninstallingKey={uninstallingKey}
            restoringKey={restoringKey}
            restoreProgressMap={restoreProgressMap}
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
            viapInstallPath={viapInstallPath || undefined}
            operationsLocked={uninstallingKey !== null}
            scanPhase={scanPhase}
            scanTotalCount={scanTotalCount}
          />
      </div>

      {/* 迁移进度弹窗 */}
      <MigrationModal
        isOpen={migrationModalOpen}
        step={migrationStep}
        appName={
          batchMigrating && batchProgress.total > 0
            ? `批量迁移 (${batchProgress.current}/${batchProgress.total}) — ${migratingApp?.display_name || ''}`
            : migratingApp?.display_name || ''
        }
        message={migrationMessage}
        lockedProcesses={lockedProcesses}
        progress={migrationProgress}
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

      {/* 强制删除预览弹窗 — 列出安装目录内容，用户确认后执行 force_remove */}
      <CleanupModal
        isOpen={forceRmPreviewOpen}
        appName={forceRmPreviewApp?.display_name || ''}
        items={forceRmPreviewItems}
        loading={forceRmLoading}
        scanning={false}
        onClose={() => {
          if (!forceRmLoading) {
            setForceRmPreviewOpen(false);
            setForceRmPreviewApp(null);
            setForceRmPreviewItems([]);
            // 用户取消强制删除预览后，释放此前为整个卸载流程设置的全局操作锁。
            setUninstallingKey(null);
          }
        }}
        onToggleItem={handleForceRmToggleItem}
        onConfirm={handleForceRmConfirm}
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

      {/* 高风险路径确认弹窗（WARNING 级别） */}
      {warningDialog && (
        <WarningConfirmDialog
          isOpen={warningDialog.isOpen}
          warningInfo={warningDialog.warningInfo}
          onConfirm={() => warningDialog.resolve(true)}
          onCancel={() => warningDialog.resolve(false)}
        />
      )}

      {/* Toast 根据通知类型自动选择停留时间，错误提示默认更久。 */}
      <Toast
        message={toast.message}
        type={toast.type}
        visible={toast.visible}
        duration={toast.duration}
        onClose={hideToast}
      />
    </div>
  );
}
