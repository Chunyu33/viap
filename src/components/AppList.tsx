// 应用列表组件 — 桌面工具风格
// 表格化行布局，紧凑信息密度，弱化操作按钮视觉

import { Package, Search, X, Link2, Check, ArrowRightLeft, FolderOpen, RotateCw, LoaderCircle, ArrowUpDown, ArrowUp, ArrowDown, ShieldCheck, ShieldOff } from 'lucide-react';
import { InstalledApp } from '../types';
import { useState, useMemo, useDeferredValue, memo, useEffect } from 'react';
import FilterSelect from './FilterSelect';
import EmptyState from './EmptyState';
import { useViapStore } from '../store';
import { useDangerousPathCheck } from '../hooks/useDangerousPathCheck';

type MigrationFilter = 'all' | 'migrated' | 'not_migrated';
type DriveFilter = 'all' | 'c' | 'other';

function extractDriveLetters(apps: InstalledApp[]): string[] {
  const drives = new Set<string>();
  for (const app of apps) {
    const match = app.install_location.match(/^([A-Za-z]):/i);
    if (match) drives.add(match[1].toUpperCase());
  }
  return Array.from(drives).sort();
}

interface AppListProps {
  apps: InstalledApp[];
  loading: boolean;
  onMigrate: (app: InstalledApp) => void;
  onRestore: (app: InstalledApp) => void;
  onUninstall: (app: InstalledApp) => void;
  onOpenFolder?: (app: InstalledApp) => void;
  uninstallingKey?: string | null;
  restoringKey?: string | null;
  restoreProgressMap?: Record<string, number>;
  migratedPaths?: string[];
  selectedKeys?: Set<string>;
  onToggleSelect?: (app: InstalledApp) => void;
  onSelectAll?: () => void;
  onBatchMigrate?: () => void;
  onStopBatchMigrate?: () => void;
  batchMigrating?: boolean;
  batchProgress?: { current: number; total: number };
  sizesLoading?: boolean;
  sizeMap?: Map<string, number>;
  onRefresh?: () => void;
  refreshing?: boolean;
  /** Viap 自身的安装目录，用于禁用自身的迁移/卸载按钮 */
  viapInstallPath?: string;
  /** 流式扫描阶段 */
  scanPhase?: 'idle' | 'snapshot' | 'tier1' | 'tier2' | 'tier3' | 'icons' | 'sizes' | 'sizes_done' | 'done';
  /** 当前累计扫描到的应用数 */
  scanTotalCount?: number;
}

function formatSize(kb: number): string {
  if (kb === 0) return '—'; // em dash
  if (kb < 1024) return `${kb} KB`;
  if (kb < 1024 * 1024) return `${(kb / 1024).toFixed(1)} MB`;
  return `${(kb / (1024 * 1024)).toFixed(2)} GB`;
}

function AppIconFallback({ app, iconsLoading }: { app: InstalledApp; iconsLoading?: boolean }) {
  const initial = app.display_name.charAt(0).toUpperCase();
  const hue = (app.display_name.charCodeAt(0) * 37) % 360;

  // 图标仍在加载中时保留轻量动效，避免用户误以为列表卡住。
  if (iconsLoading) {
    return (
      <div className="w-7 h-7 rounded-full flex items-center justify-center flex-shrink-0 relative">
        <div
          className="absolute inset-0 rounded-full animate-spin"
          style={{
            border: '2px solid transparent',
            borderTopColor: `hsl(${hue}, 55%, 55%)`,
            borderRightColor: `hsl(${hue}, 55%, 55%)`,
          }}
        />
        <span
          className="text-[11px] font-semibold"
          style={{ color: `hsl(${hue}, 55%, 55%)`, opacity: 0.5 }}
        >
          {initial}
        </span>
      </div>
    );
  }

  return (
    <div
      className="w-7 h-7 rounded flex items-center justify-center flex-shrink-0 text-[11px] font-semibold text-white"
      style={{ background: `hsl(${hue}, 55%, 55%)` }}
      title="未能提取应用图标，显示首字母占位"
    >
      {initial}
    </div>
  );
}

function AppIcon({ app, iconsLoading }: { app: InstalledApp; iconsLoading?: boolean }) {
  const [imageFailed, setImageFailed] = useState(false);
  const iconSource = app.icon_url || app.icon_base64;
  if (iconSource && !imageFailed) {
    return (
      <div
        className="w-7 h-7 rounded flex items-center justify-center flex-shrink-0 overflow-hidden"
        style={{ background: 'var(--color-gray-100)' }}
      >
        <img
          src={iconSource}
          alt=""
          className="w-5 h-5 object-contain"
          // 有些卸载器/辅助 exe 没有可提取图标，失败后切换到稳定占位而不是留下灰块。
          onError={() => setImageFailed(true)}
        />
      </div>
    );
  }
  return <AppIconFallback app={app} iconsLoading={iconsLoading} />;
}

const AppRow = memo(function AppRow({
  app, onMigrate, onRestore, onUninstall, onOpenFolder,
  isUninstalling, isMigrated, isRestoring,
  restoreProgress,
  isSelected, onToggleSelect, showCheckbox,
  appSize, isViap, iconsLoading,
}: {
  app: InstalledApp;
  onMigrate: (app: InstalledApp) => void;
  onRestore: (app: InstalledApp) => void;
  onUninstall: (app: InstalledApp) => void;
  onOpenFolder: (app: InstalledApp) => void;
  isUninstalling: boolean;
  isMigrated: boolean;
  isRestoring: boolean;
  restoreProgress?: number;
  isSelected?: boolean;
  onToggleSelect?: (app: InstalledApp) => void;
  showCheckbox?: boolean;
  appSize?: number;
  isViap?: boolean;
  iconsLoading?: boolean;
}) {
  const rowStyle: React.CSSProperties = {
    height: 'var(--row-height)' as unknown as string,
    padding: '0 8px',
    background: isSelected ? 'var(--bg-row-selected)' : 'transparent',
    borderBottom: '1px solid var(--border-color)',
  } as React.CSSProperties;

  return (
    <div
      className="flex items-center gap-3 transition-colors relative"
      style={rowStyle}
      onMouseEnter={(e) => {
        if (!isSelected) (e.currentTarget as HTMLElement).style.background = 'var(--bg-row-hover)';
      }}
      onMouseLeave={(e) => {
        if (!isSelected) (e.currentTarget as HTMLElement).style.background = 'transparent';
      }}
    >
      {/* checkbox */}
      {showCheckbox && !isMigrated && !isViap && (
        <button
          onClick={(e) => { e.stopPropagation(); onToggleSelect?.(app); }}
          className={`flex-shrink-0 w-4 h-4 rounded-sm border flex items-center justify-center ${
            isSelected
              ? ''
              : 'border-[var(--border-color-strong)] opacity-60 hover:opacity-100'
          }`}
          style={isSelected ? {
            background: 'var(--color-primary)',
            borderColor: 'var(--color-primary)',
          } : undefined}
        >
          {isSelected && <Check className="w-3 h-3 text-white" strokeWidth={3} />}
        </button>
      )}
      {(showCheckbox && (isMigrated || isViap)) && <div className="flex-shrink-0 w-4 h-4" />}

      {/* left bar for migrated */}
      {isMigrated && (
        <div
          className="absolute left-0 top-0 bottom-0 w-0.5"
          style={{ background: 'var(--color-primary)' }}
        />
      )}

      {/* icon */}
      <AppIcon app={app} iconsLoading={iconsLoading} />

      {/* name + path */}
      <div className="flex-1 min-w-0 flex items-center gap-4">
        <div className="flex items-center gap-2 min-w-0" style={{ maxWidth: '280px' }}>
          <span
            className="text-[13px] font-medium truncate"
            style={{ color: 'var(--text-primary)' }}
          >
            {app.display_name}
          </span>
          {isMigrated && (
            <span className="badge badge-success flex-shrink-0">
              <Link2 className="w-2.5 h-2.5" />
              已迁移
            </span>
          )}
        </div>
        <span
          className="text-[11px] truncate flex-1 min-w-0 hidden sm:block"
          style={{ color: 'var(--text-tertiary)' }}
          title={app.install_location}
        >
          {app.install_location}
        </span>
      </div>

      {/* size */}
      <span
        className="text-[11px] tabular-nums flex-shrink-0 w-16 text-right"
        style={{ color: 'var(--text-secondary)' }}
      >
        {formatSize(appSize ?? 0)}
      </span>

      {/* actions */}
      <div className="flex items-center gap-1 flex-shrink-0" style={{ width: '150px', justifyContent: 'flex-end' }}>
        <button
          onClick={() => onOpenFolder(app)}
          className="btn btn-ghost btn-icon"
          title="打开目录"
        >
          <FolderOpen className="w-3.5 h-3.5" />
        </button>

        {isMigrated ? (
          <button
            onClick={() => onRestore(app)}
            disabled={isRestoring}
            className="btn btn-sm h-6 text-[11px]"
            style={isRestoring ? {
              // 直接在按钮背景绘制进度条，列表行无需额外占用空间。
              background: `linear-gradient(to right, var(--color-primary-light) 0%, var(--color-primary-light) ${Math.round(restoreProgress ?? 0)}%, transparent ${Math.round(restoreProgress ?? 0)}%)`,
              borderColor: 'var(--color-primary)',
              color: 'var(--color-primary)',
            } : undefined}
          >
            {/* 恢复过程由后端推送百分比，按钮内直接展示，避免只显示 loading。 */}
            {isRestoring ? `${Math.round(restoreProgress ?? 0)}%` : '还原'}
          </button>
        ) : (
          <button
            onClick={() => onMigrate(app)}
            disabled={isViap}
            className="btn btn-primary btn-sm h-6 text-[11px]"
            title={isViap ? 'Viap 是当前运行的应用，不可迁移自身' : undefined}
          >
            迁移
          </button>
        )}

        <button
          onClick={() => onUninstall(app)}
          disabled={isUninstalling || isViap}
          className="btn btn-link btn-link-danger h-6 text-[11px]"
          title={isViap ? 'Viap 是当前运行的应用，不可卸载自身' : undefined}
        >
          {isUninstalling ? '卸载中...' : '卸载'}
        </button>
      </div>
    </div>
  );
});

function LoadingSkeleton() {
  const items = [1, 2, 3, 4, 5, 6, 7, 8];
  const rowStyle: React.CSSProperties = {
    height: 'var(--row-height)' as unknown as string,
    padding: '0 8px',
    borderBottom: '1px solid var(--border-color)',
  } as React.CSSProperties;

  return (
    <div className="flex flex-col">
      {items.map((i) => (
        <div key={i} className="flex items-center gap-3 animate-pulse" style={rowStyle}>
          <div className="w-4 h-4 rounded-sm" style={{ background: 'var(--bg-row-hover)' }} />
          <div className="w-7 h-7 rounded" style={{ background: 'var(--bg-row-hover)' }} />
          <div className="flex-1 min-w-0">
            <div className="h-3 rounded w-32" style={{ background: 'var(--bg-row-hover)' }} />
          </div>
          <div className="w-16 h-3 rounded" style={{ background: 'var(--bg-row-hover)' }} />
          <div className="flex gap-1" style={{ width: '130px', justifyContent: 'flex-end' }}>
            <div className="w-7 h-7 rounded" style={{ background: 'var(--bg-row-hover)' }} />
            <div className="w-12 h-7 rounded" style={{ background: 'var(--bg-row-hover)' }} />
            <div className="w-10 h-7 rounded" style={{ background: 'var(--bg-row-hover)' }} />
          </div>
        </div>
      ))}
    </div>
  );
}

export default function AppList({
  apps, loading, onMigrate, onRestore, onUninstall, onOpenFolder,
  uninstallingKey = null, restoringKey = null, migratedPaths = [],
  restoreProgressMap = {},
  selectedKeys, onToggleSelect, onSelectAll, onBatchMigrate,
  onStopBatchMigrate,
  batchMigrating = false, batchProgress,
  sizesLoading = false,
  sizeMap,
  onRefresh,
  refreshing = false,
  viapInstallPath,
  scanPhase,
  // scanTotalCount = 0,
}: AppListProps) {
  const defaultOpenFolder = async (app: InstalledApp) => {
    try {
      const { invoke } = await import('@tauri-apps/api/core');
      await invoke('open_folder', { path: app.install_location });
    } catch (error) {
      console.error('Failed to open folder:', error);
    }
  };
  const handleOpenFolder = onOpenFolder ?? defaultOpenFolder;
  // ── Zustand 全局 UI 状态：跨 Tab 保持 ──
  const searchQuery = useViapStore((s) => s.searchQuery);
  const setSearchQuery = useViapStore((s) => s.setSearchQuery);
  const migrationFilter = useViapStore((s) => s.migrationFilter);
  const setMigrationFilter = useViapStore((s) => s.setMigrationFilter);
  const driveFilter = useViapStore((s) => s.driveFilter as DriveFilter);
  const setDriveFilter = useViapStore((s) => s.setDriveFilter);
  const sortKey = useViapStore((s) => s.sortKey);
  const sortOrder = useViapStore((s) => s.sortOrder);
  const setSort = useViapStore((s) => s.setSort);
  const resetUI = useViapStore((s) => s.resetUI);

  // BLOCKED 应用显隐切换：默认隐藏，仅展示可安全操作的应用
  const [showBlockedApps, setShowBlockedApps] = useState(false);
  const { isBlockedPath } = useDangerousPathCheck();

  const [inputQuery, setInputQuery] = useState(searchQuery);
  const deferredSearchQuery = useDeferredValue(inputQuery);

  // 同步 inputQuery → store.searchQuery（用于跨 Tab 恢复）
  useEffect(() => { setSearchQuery(inputQuery); }, [inputQuery, setSearchQuery]);

  // 刷新时重置排序状态
  useEffect(() => {
    if (refreshing) { resetUI(); }
  }, [refreshing, resetUI]);
  const migratedPathSet = useMemo(
    () => new Set(migratedPaths.map((path) => path.toLowerCase())),
    [migratedPaths],
  );

  const isAppMigrated = (app: InstalledApp): boolean =>
    migratedPathSet.has(app.install_location.toLowerCase());

  const isViapSelfApp = (app: InstalledApp): boolean =>
    viapInstallPath
      ? app.install_location.toLowerCase().replace(/\//g, '\\') ===
        viapInstallPath.toLowerCase().replace(/\//g, '\\')
      : false;

  // 可见应用（受 BLOCKED 过滤影响）：用于筛选下拉选项的动态列表
  // 避免隐藏的 BLOCKED 应用所在盘符仍出现在盘符筛选中
  const visibleApps = useMemo(() => {
    if (showBlockedApps) return apps;
    return apps.filter(app => !isBlockedPath(app.install_location));
  }, [apps, showBlockedApps, isBlockedPath]);

  const availableDrives = useMemo(() => extractDriveLetters(visibleApps), [visibleApps]);
  const otherDrives = useMemo(() => availableDrives.filter(d => d !== 'C'), [availableDrives]);

  const filteredApps = useMemo(() => {
    const q = deferredSearchQuery.trim().toLowerCase();
    return apps.filter(app => {
      if (q && !app.display_name.toLowerCase().includes(q) && !app.install_location.toLowerCase().includes(q)) {
        return false;
      }
      if (migrationFilter !== 'all') {
        const migrated = migratedPathSet.has(app.install_location.toLowerCase());
        if (migrationFilter === 'migrated' && !migrated) return false;
        if (migrationFilter === 'not_migrated' && migrated) return false;
      }
      if (driveFilter !== 'all') {
        const dl = app.install_location.charAt(0).toUpperCase();
        if (driveFilter === 'c' && dl !== 'C') return false;
        if (driveFilter === 'other' && dl === 'C') return false;
      }
      // BLOCKED 应用默认隐藏（系统目录/浏览器/GPU 驱动等不可迁移项）
      if (!showBlockedApps && isBlockedPath(app.install_location)) {
        return false;
      }
      return true;
    });
  }, [apps, deferredSearchQuery, migrationFilter, driveFilter, migratedPathSet, showBlockedApps, isBlockedPath]);

  // 排序点击处理：同 key 三态切换 asc → desc → 清除
  const handleSort = (key: 'name' | 'size') => {
    if (sortKey === key) {
      if (sortOrder === 'asc') {
        setSort(key, 'desc');
      } else {
        setSort(null, 'asc');
      }
    } else {
      setSort(key, 'asc');
    }
  };

  // 本地内存排序，不触发任何后端调用
  const sortedApps = useMemo(() => {
    if (!sortKey) return filteredApps;
    const sorted = [...filteredApps];
    sorted.sort((a, b) => {
      let cmp: number;
      if (sortKey === 'name') {
        cmp = a.display_name.localeCompare(b.display_name, 'zh-CN');
      } else {
        const keyA = a.registry_path || a.install_location;
        const keyB = b.registry_path || b.install_location;
        const sizeA = sizeMap?.get(keyA) ?? sizeMap?.get(a.install_location.toLowerCase()) ?? 0;
        const sizeB = sizeMap?.get(keyB) ?? sizeMap?.get(b.install_location.toLowerCase()) ?? 0;
        cmp = sizeA - sizeB;
      }
      return sortOrder === 'desc' ? -cmp : cmp;
    });
    return sorted;
  }, [filteredApps, sortKey, sortOrder, sizeMap]);

  // 根据当前筛选/搜索结果聚合大小，跟随过滤条件实时变化
  const filteredTotalSize = useMemo(() => {
    if (!sizeMap || sizeMap.size === 0) return 0;
    let total = 0;
    for (const app of filteredApps) {
      const key = app.registry_path || app.install_location;
      total += sizeMap.get(key) ?? sizeMap.get(app.install_location.toLowerCase()) ?? 0;
    }
    return total;
  }, [filteredApps, sizeMap]);

  const migrationOptions: { value: MigrationFilter; label: string }[] = [
    { value: 'all', label: '全部' },
    { value: 'migrated', label: '已迁移' },
    { value: 'not_migrated', label: '未迁移' },
  ];

  const selectableCount = useMemo(
    () => filteredApps.filter(a => !isAppMigrated(a) && !isViapSelfApp(a)).length,
    [filteredApps, migratedPathSet, viapInstallPath],
  );

  const driveOptions: { value: DriveFilter; label: string }[] = [
    { value: 'all', label: '全部盘' },
    { value: 'c', label: 'C 盘' },
    { value: 'other', label: `其他盘${otherDrives.length > 0 ? ` (${otherDrives.join('/')})` : ''}` },
  ];

  if (loading) {
    const loadingHint = '正在扫描应用...';
    return (
      <div className="flex-1 flex flex-col">
        <div
          className="flex items-center gap-2 mb-2 text-[12px]"
          style={{ color: 'var(--text-tertiary)' }}
        >
          <div className="w-3.5 h-3.5 border-2 border-[var(--color-primary)] border-t-transparent rounded-full animate-spin" />
          {loadingHint}
        </div>
        <LoadingSkeleton />
      </div>
    );
  }

  if (apps.length === 0) {
    return (
      <EmptyState icon={<Package />} title="未找到可迁移的应用" description="系统扫描未发现已安装的应用" />
    );
  }

  return (
    <div className="flex-1 flex flex-col min-h-0">
      {/* toolbar */}
      <div className="flex items-center gap-2 flex-shrink-0 mb-1" style={{ padding: '2px 8px' }}>
        <div className="relative flex-1 max-w-xs">
          <Search
            className="absolute left-2 top-1/2 -translate-y-1/2 w-3.5 h-3.5"
            style={{ color: 'var(--text-tertiary)' }}
          />
          <input
            type="text"
            placeholder="搜索应用..."
            value={inputQuery}
            onChange={(e) => setInputQuery(e.target.value)}
            className="w-full h-8 pl-7 pr-7 text-[12px] rounded border outline-none transition-colors"
            style={{
              background: 'var(--bg-input)',
              borderColor: 'var(--border-color)',
              color: 'var(--text-primary)',
            }}
            onFocus={(e) => { e.currentTarget.style.borderColor = 'var(--color-primary)'; }}
            onBlur={(e) => { e.currentTarget.style.borderColor = 'var(--border-color)'; }}
          />
          {inputQuery && (
            <button
              onClick={() => setInputQuery('')}
              className="absolute right-1.5 top-1/2 -translate-y-1/2 w-4 h-4 flex items-center justify-center rounded-sm"
              style={{ color: 'var(--text-tertiary)' }}
              onMouseEnter={(e) => { (e.currentTarget as HTMLElement).style.color = 'var(--text-primary)'; }}
              onMouseLeave={(e) => { (e.currentTarget as HTMLElement).style.color = 'var(--text-tertiary)'; }}
            >
              <X className="w-3 h-3" />
            </button>
          )}
        </div>
        <FilterSelect
          value={migrationFilter}
          onChange={setMigrationFilter}
          options={migrationOptions}
          className="w-[120px]"
        />
        <FilterSelect
          value={driveFilter}
          onChange={setDriveFilter}
          options={driveOptions}
          className="w-[120px]"
        />
        {/* BLOCKED 应用显隐切换 + 刷新 */}
        <span className="text-[11px] flex-shrink-0 ml-1 flex items-center gap-1">
          <button
            onClick={() => setShowBlockedApps(!showBlockedApps)}
            className="flex items-center justify-center h-8 w-8 rounded border cursor-pointer transition-colors"
            style={{
              background: 'transparent',
              borderColor: 'var(--border-color)',
            }}
            title={showBlockedApps ? '隐藏不可迁移的系统应用' : '显示所有应用（含不可迁移项）'}
            onMouseEnter={(e) => { (e.currentTarget as HTMLElement).style.background = 'var(--bg-row-hover)'; }}
            onMouseLeave={(e) => { (e.currentTarget as HTMLElement).style.background = 'transparent'; }}
          >
            {showBlockedApps
              ? <ShieldOff className="w-3.5 h-3.5" style={{ color: 'var(--color-warning)' }} />
              : <ShieldCheck className="w-3.5 h-3.5" style={{ color: 'var(--color-primary)' }} />
            }
          </button>
          {onRefresh && (
            <button
              onClick={onRefresh}
              className="flex items-center justify-center h-8 w-8 rounded border cursor-pointer transition-colors"
              style={{
                background: 'transparent',
                borderColor: 'var(--border-color)',
              }}
              title="刷新应用列表"
              disabled={refreshing}
              onMouseEnter={(e) => { if (!refreshing) (e.currentTarget as HTMLElement).style.background = 'var(--bg-row-hover)'; }}
              onMouseLeave={(e) => { (e.currentTarget as HTMLElement).style.background = 'transparent'; }}
            >
              <RotateCw className={`w-3 h-3 ${refreshing ? 'animate-spin' : ''}`} />
            </button>
          )}
        </span>

        {onToggleSelect && onSelectAll && onBatchMigrate && (
          <div className="flex items-center gap-2 ml-auto">
            <button onClick={onSelectAll} className="text-[11px] btn-link">
              {selectableCount > 0 && selectedKeys && selectedKeys.size === selectableCount
                ? '取消全选'
                : '全选未迁移'}
            </button>
            {batchMigrating ? (
              <button
                onClick={onStopBatchMigrate}
                className="btn h-7 text-[11px]"
                style={{ background: 'var(--color-danger)', color: 'var(--text-inverse)', borderColor: 'var(--color-danger)' }}
              >
                {batchProgress
                  ? `停止 (${batchProgress.current}/${batchProgress.total})`
                  : '停止'}
              </button>
            ) : (
              <button
                onClick={onBatchMigrate}
                disabled={!selectedKeys || selectedKeys.size === 0}
                className="btn btn-primary h-7 text-[11px]"
                style={{
                  visibility: selectedKeys && selectedKeys.size > 0 ? 'visible' : 'hidden',
                }}
              >
                <ArrowRightLeft className="w-3 h-3" />
                批量迁移 ({selectedKeys?.size ?? 0})
              </button>
            )}
          </div>
        )}
      </div>

      {/* column header */}
      <div
        className="flex items-center gap-3 flex-shrink-0 text-[10px] uppercase tracking-wider"
        style={{
          padding: '0 8px',
          height: '24px',
          color: 'var(--text-tertiary)',
          borderBottom: '1px solid var(--border-color-strong)',
        }}
      >
        <div className="flex-shrink-0 w-4" />
        <div className="flex-shrink-0 w-7" />
        <button
          className="flex-1 min-w-0 flex items-center gap-1 cursor-pointer hover:text-[var(--text-primary)] transition-colors"
          onClick={() => handleSort('name')}
          style={{ background: 'none', border: 'none', padding: 0, color: 'inherit', font: 'inherit' }}
        >
          名称
          {sortKey === 'name' ? (
            sortOrder === 'asc'
              ? <ArrowUp className="h-3 w-3" style={{ color: 'var(--color-primary)' }} />
              : <ArrowDown className="h-3 w-3" style={{ color: 'var(--color-primary)' }} />
          ) : (
            <ArrowUpDown className="h-3 w-3 opacity-30" />
          )}
        </button>
        <button
          className="flex-shrink-0 w-16 flex items-center justify-end gap-0.5 cursor-pointer hover:text-[var(--text-primary)] transition-colors"
          onClick={() => handleSort('size')}
          style={{ background: 'none', border: 'none', padding: 0, color: 'inherit', font: 'inherit' }}
        >
          大小
          {sortKey === 'size' ? (
            sortOrder === 'asc'
              ? <ArrowUp className="h-3 w-3" style={{ color: 'var(--color-primary)' }} />
              : <ArrowDown className="h-3 w-3" style={{ color: 'var(--color-primary)' }} />
          ) : (
            <ArrowUpDown className="h-3 w-3 opacity-30" />
          )}
        </button>
        <span className="flex-shrink-0" style={{ width: '150px', textAlign: 'right' }}>操作</span>
      </div>

      {/* 扫描/刷新进度提示：仅在进行中显示，不遮挡已加载的应用 */}
      {((scanPhase && scanPhase !== 'done') || refreshing) && (
        <div
          className="flex items-center gap-2 px-3 py-1.5 text-xs rounded-md mb-1 flex-shrink-0"
          style={{ background: 'var(--color-primary-light)', color: 'var(--color-primary)' }}
        >
          <LoaderCircle className="h-3 w-3 animate-spin flex-shrink-0" />
          <span>
            {refreshing && '正在刷新应用列表...'}
            {!refreshing && scanPhase === 'snapshot' && `已读取上次快照，正在校验应用列表...`}
            {!refreshing && scanPhase === 'tier1' && `正在扫描快捷方式...`}
            {!refreshing && scanPhase === 'tier2' && `正在扫描文件系统...`}
            {!refreshing && scanPhase === 'tier3' && `正在加载图标...`}
            {!refreshing && scanPhase === 'icons' && `正在加载图标...`}
            {!refreshing && scanPhase === 'sizes' && `正在计算目录大小...`}
          </span>
        </div>
      )}

      {/* list body */}
      <div className="flex-1 min-h-0 overflow-y-auto">
        {sortedApps.length > 0 ? (
          <div className="flex flex-col">
            {sortedApps.map((app) => {
              const key = app.registry_path || app.install_location;
              const isViapSelf = isViapSelfApp(app);
              // 流式扫描完成前图标尚未全部加载
              const iconsLoading = !!scanPhase && scanPhase !== 'done' && scanPhase !== 'idle';
              return (
                <AppRow
                  key={key}
                  app={app}
                  iconsLoading={iconsLoading}
                  onMigrate={onMigrate}
                  onRestore={onRestore}
                  onUninstall={onUninstall}
                  onOpenFolder={handleOpenFolder}
                  isUninstalling={uninstallingKey === `${app.display_name}|${app.registry_path}`}
                  isRestoring={restoringKey === `${app.display_name}|${app.registry_path}`}
                  restoreProgress={restoreProgressMap[`${app.display_name}|${app.registry_path}`]}
                  isMigrated={isAppMigrated(app)}
                  isSelected={selectedKeys?.has(key)}
                  onToggleSelect={onToggleSelect}
                  showCheckbox={!!onToggleSelect}
                  // 后台线程以 install_location 为 key 推送大小
                  // 兼容 registry-scanned 应用（key = registry_path）和非注册表应用（key = install_location）
                  appSize={sizeMap?.get(key) ?? sizeMap?.get(app.install_location.toLowerCase())}
                  isViap={isViapSelf}
                />
              );
            })}
          </div>
        ) : (
          <div className='flex justify-center items-center w-full h-full'>
            <EmptyState icon={<Search />} title="未找到匹配的应用" description="尝试调整筛选条件或搜索关键词" />
          </div>
        )}
      </div>

      {/* footer: 应用总数 + 总占用 */}
      <div
        className="flex-shrink-0 flex items-center gap-2 text-[12px]"
        style={{
          padding: '8px 0',
          color: 'var(--text-secondary)',
          borderTop: '1px solid var(--border-color)',
        }}
      >
        <span className="tabular-nums" style={{ color: 'var(--text-primary)' }}>
          <span className="mr-1 font-bold" style={{ color: 'var(--text-primary)' }}>
            {filteredApps.length}
          </span>
          个应用
        </span>
        <span style={{ color: 'var(--border-color-strong)' }}>·</span>
        {sizesLoading ? (
          <span className="inline-block w-3 h-3 border border-[var(--color-primary)] border-t-transparent rounded-full animate-spin" />
        ) : (
          <>
            <span>总占用</span>
            <span className="tabular-nums font-bold" style={{ color: 'var(--text-primary)' }}>
              {formatSize(filteredTotalSize)}
            </span>
          </>
        )}
      </div>
    </div>
  );
}
