// 应用列表组件 — 桌面工具风格
// 表格化行布局，紧凑信息密度，弱化操作按钮视觉

import { Package, Search, X, Link2, Check, ArrowRightLeft, FolderOpen, RotateCw } from 'lucide-react';
import { InstalledApp } from '../types';
import { useState, useMemo, useDeferredValue, memo, useEffect } from 'react';
import FilterSelect from './FilterSelect';
import EmptyState from './EmptyState';

type MigrationFilter = 'all' | 'migrated' | 'not_migrated';
type DriveFilter = 'all' | 'c' | 'other';

// 模块级状态缓存：Tab 切换时 AppList 被卸载，搜索/筛选条件保留在此
// 下次挂载时自动恢复，用户无感知
let cachedSearchQuery = '';
let cachedMigrationFilter: MigrationFilter = 'all';
let cachedDriveFilter: DriveFilter = 'all';

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
}

function formatSize(kb: number): string {
  if (kb === 0) return '—'; // em dash
  if (kb < 1024) return `${kb} KB`;
  if (kb < 1024 * 1024) return `${(kb / 1024).toFixed(1)} MB`;
  return `${(kb / (1024 * 1024)).toFixed(2)} GB`;
}

function AppIcon({ app }: { app: InstalledApp }) {
  // icon_base64 是当前稳定方案，icon_url 预留后续自定义协议迁移
  if (app.icon_base64) {
    return (
      <div
        className="w-7 h-7 rounded flex items-center justify-center flex-shrink-0 overflow-hidden"
        style={{ background: 'var(--color-gray-100)' }}
      >
        <img
          src={app.icon_base64}
          alt=""
          className="w-5 h-5 object-contain"
          onError={(e) => { (e.target as HTMLImageElement).style.display = 'none'; }}
        />
      </div>
    );
  }
  const initial = app.display_name.charAt(0).toUpperCase();
  const hue = (app.display_name.charCodeAt(0) * 37) % 360;
  return (
    <div
      className="w-7 h-7 rounded flex items-center justify-center flex-shrink-0 text-[11px] font-semibold text-white"
      style={{ background: `hsl(${hue}, 55%, 55%)` }}
    >
      {initial}
    </div>
  );
}

const AppRow = memo(function AppRow({
  app, onMigrate, onRestore, onUninstall, onOpenFolder,
  isUninstalling, isMigrated, isRestoring,
  isSelected, onToggleSelect, showCheckbox,
  appSize, isViap,
}: {
  app: InstalledApp;
  onMigrate: (app: InstalledApp) => void;
  onRestore: (app: InstalledApp) => void;
  onUninstall: (app: InstalledApp) => void;
  onOpenFolder: (app: InstalledApp) => void;
  isUninstalling: boolean;
  isMigrated: boolean;
  isRestoring: boolean;
  isSelected?: boolean;
  onToggleSelect?: (app: InstalledApp) => void;
  showCheckbox?: boolean;
  appSize?: number;
  isViap?: boolean;
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
      {showCheckbox && !isMigrated && (
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
      {showCheckbox && isMigrated && <div className="flex-shrink-0 w-4 h-4" />}

      {/* left bar for migrated */}
      {isMigrated && (
        <div
          className="absolute left-0 top-0 bottom-0 w-0.5"
          style={{ background: 'var(--color-primary)' }}
        />
      )}

      {/* icon */}
      <AppIcon app={app} />

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
          >
            {isRestoring ? '还原中...' : '还原'}
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
  selectedKeys, onToggleSelect, onSelectAll, onBatchMigrate,
  onStopBatchMigrate,
  batchMigrating = false, batchProgress,
  sizesLoading = false,
  sizeMap,
  onRefresh,
  refreshing = false,
  viapInstallPath,
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
  const [inputQuery, setInputQuery] = useState(cachedSearchQuery);
  const [migrationFilter, setMigrationFilter] = useState<MigrationFilter>(cachedMigrationFilter);
  const [driveFilter, setDriveFilter] = useState<DriveFilter>(cachedDriveFilter);
  const deferredSearchQuery = useDeferredValue(inputQuery);

  // 将搜索/筛选状态同步到模块级缓存，跨越 Tab 切换保持
  useEffect(() => { cachedSearchQuery = inputQuery; }, [inputQuery]);
  useEffect(() => { cachedMigrationFilter = migrationFilter; }, [migrationFilter]);
  useEffect(() => { cachedDriveFilter = driveFilter; }, [driveFilter]);
  const migratedPathSet = useMemo(
    () => new Set(migratedPaths.map((path) => path.toLowerCase())),
    [migratedPaths],
  );

  const isAppMigrated = (app: InstalledApp): boolean =>
    migratedPathSet.has(app.install_location.toLowerCase());

  const availableDrives = useMemo(() => extractDriveLetters(apps), [apps]);
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
      return true;
    });
  }, [apps, deferredSearchQuery, migrationFilter, driveFilter, migratedPathSet]);

  // 根据当前筛选/搜索结果聚合大小，跟随过滤条件实时变化
  const filteredTotalSize = useMemo(() => {
    if (!sizeMap || sizeMap.size === 0) return 0;
    let total = 0;
    for (const app of filteredApps) {
      const key = app.registry_path || app.install_location;
      total += sizeMap.get(key) ?? 0;
    }
    return total;
  }, [filteredApps, sizeMap]);

  const migrationOptions: { value: MigrationFilter; label: string }[] = [
    { value: 'all', label: '全部状态' },
    { value: 'migrated', label: '已迁移' },
    { value: 'not_migrated', label: '未迁移' },
  ];

  const selectableCount = useMemo(
    () => filteredApps.filter(a => !isAppMigrated(a)).length,
    [filteredApps, migratedPathSet],
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
        <span
          className="text-[11px] flex-shrink-0 ml-1 flex items-center gap-1"
          style={{ color: 'var(--text-tertiary)' }}
        >
          {filteredApps.length} 个
          {onRefresh && (
            <button
              onClick={onRefresh}
              className="btn btn-ghost btn-icon w-5 h-5 flex items-center justify-center"
              title="刷新应用列表"
              disabled={refreshing}
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
        <span className="flex-1 min-w-0">名称</span>
        <span className="flex-shrink-0 w-16 text-right">大小</span>
        <span className="flex-shrink-0" style={{ width: '150px', textAlign: 'right' }}>操作</span>
      </div>

      {/* list body */}
      <div className="flex-1 min-h-0 overflow-y-auto">
        {filteredApps.length > 0 ? (
          <div className="flex flex-col">
            {filteredApps.map((app) => {
              const key = app.registry_path || app.install_location;
              // 判断当前应用是否为 Viap 自身（大小写不敏感）
              const isViapSelf = viapInstallPath
                ? app.install_location.toLowerCase().replace(/\//g, '\\') ===
                  viapInstallPath.toLowerCase().replace(/\//g, '\\')
                : false;
              return (
                <AppRow
                  key={key}
                  app={app}
                  onMigrate={onMigrate}
                  onRestore={onRestore}
                  onUninstall={onUninstall}
                  onOpenFolder={handleOpenFolder}
                  isUninstalling={uninstallingKey === `${app.display_name}|${app.registry_path}`}
                  isRestoring={restoringKey === `${app.display_name}|${app.registry_path}`}
                  isMigrated={isAppMigrated(app)}
                  isSelected={selectedKeys?.has(key)}
                  onToggleSelect={onToggleSelect}
                  showCheckbox={!!onToggleSelect}
                  appSize={sizeMap?.get(key)}
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

      {/* footer: 总占用 — 大小数据与列表解耦，仅在此处渲染一次 */}
      <div
        className="flex-shrink-0 flex items-center gap-2 text-[12px]"
        style={{
          padding: '8px 0',
          color: 'var(--text-secondary)',
          borderTop: '1px solid var(--border-color)',
        }}
      >
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
