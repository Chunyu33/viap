// 迁移记录页面 — 桌面工具风格
// 表格化行布局，紧凑信息密度

import { useEffect, useState, useMemo } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';
import {
  History, RotateCcw, RefreshCw, Loader2,
  FolderArchive, AppWindow, ArrowRight, CheckCircle2, AlertTriangle,
  Search, X, ChevronDown, ChevronUp, ArrowUpDown, ArrowUp, ArrowDown, Trash2,
} from 'lucide-react';
import { MigrationProgressEvent, MigrationRecord, MigrationResult } from '../types';
import Toast, { useToast } from '../components/Toast';
import FilterSelect from '../components/FilterSelect';
import EmptyState from '../components/EmptyState';
import { useViapStore } from '../store';

// broken_fixable: Junction 损坏但 target 仍存在（通常是用户手动绕过 Viap 操作导致）
// broken_lost: target 已不存在，数据已丢失，只能清理
type LinkStatus = 'checking' | 'healthy' | 'broken_fixable' | 'broken_lost' | 'unknown';

function formatSize(bytes: number): string {
  if (bytes === 0) return '--';
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 * 1024 * 1024) return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
  return `${(bytes / (1024 * 1024 * 1024)).toFixed(2)} GB`;
}

function formatDate(timestamp: number): string {
  const d = new Date(timestamp);
  return `${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')} ${String(d.getHours()).padStart(2, '0')}:${String(d.getMinutes()).padStart(2, '0')}`;
}

function formatFullDate(timestamp: number): string {
  return new Date(timestamp).toLocaleString('zh-CN', {
    year: 'numeric', month: '2-digit', day: '2-digit',
    hour: '2-digit', minute: '2-digit', second: '2-digit',
  });
}

function shortenPath(path: string): string {
  if (path.length <= 36) return path;
  const parts = path.split('\\');
  if (parts.length <= 2) return path;
  return `${parts[0]}\\...\\${parts.slice(-2).join('\\')}`;
}

/** 列头排序按钮 — 三态切换 asc → desc → 清除 */
type SortKey = 'name' | 'path' | 'size';
type SortBy = 'date_desc' | 'date_asc' | 'size_desc' | 'size_asc' | 'name_asc' | 'name_desc' | 'path_asc' | 'path_desc';

function SortHeader({ label, sortKey, sortBy, onSort, style }: {
  label: string;
  sortKey: SortKey;
  sortBy: SortBy;
  onSort: (key: SortKey) => void;
  style?: React.CSSProperties;
}) {
  const isActive = sortBy.startsWith(sortKey);
  const isAsc = sortBy.endsWith('_asc');

  return (
    <button
      onClick={() => onSort(sortKey)}
      className="flex items-center gap-0.5 cursor-pointer border-none bg-transparent uppercase tracking-wider"
      style={{ ...style, color: isActive ? 'var(--color-primary)' : 'var(--text-tertiary)', fontSize: '10px' }}
    >
      {label}
      {isActive
        ? (isAsc ? <ArrowUp className="w-2.5 h-2.5" /> : <ArrowDown className="w-2.5 h-2.5" />)
        : <ArrowUpDown className="w-2.5 h-2.5 opacity-30" />
      }
    </button>
  );
}

// health cache
const HEALTH_CACHE_KEY = 'viap_health_cache';
const HEALTH_CACHE_TTL = 5 * 60 * 1000;

interface CachedHealth { status: LinkStatus; timestamp: number; }

function loadHealthCache(): Record<string, CachedHealth> {
  try { const raw = localStorage.getItem(HEALTH_CACHE_KEY); return raw ? JSON.parse(raw) : {}; }
  catch { return {}; }
}
function saveHealthCache(cache: Record<string, CachedHealth>) {
  try { localStorage.setItem(HEALTH_CACHE_KEY, JSON.stringify(cache)); } catch { /* quota */ }
}
function getCachedStatus(id: string): LinkStatus | null {
  const entry = loadHealthCache()[id];
  return entry && Date.now() - entry.timestamp < HEALTH_CACHE_TTL ? entry.status : null;
}
function setCachedStatus(id: string, status: LinkStatus) {
  const cache = loadHealthCache();
  cache[id] = { status, timestamp: Date.now() };
  saveHealthCache(cache);
}

function HistoryRow({
  record, onRestore, isRestoring, restoreProgress, linkStatus, onCleanup,
}: {
  record: MigrationRecord;
  onRestore: (id: string, recordType: string) => void;
  isRestoring: boolean;
  restoreProgress?: number;
  linkStatus: LinkStatus;
  onCleanup?: (id: string) => void;
}) {
  const isLargeFolder = record.record_type === 'LargeFolder';
  const [expanded, setExpanded] = useState(false);

  const rowStyle: React.CSSProperties = {
    borderBottom: '1px solid var(--border-color)',
    background: linkStatus === 'broken_lost' ? 'var(--color-danger-light)'
      : linkStatus === 'broken_fixable' ? 'var(--color-warning-light)'
      : 'transparent',
  } as React.CSSProperties;

  return (
    <div style={rowStyle}>
      <div
        className="flex items-center gap-3 cursor-pointer"
        style={{ height: 'var(--row-height)', padding: '0 8px' }}
        onClick={() => setExpanded(!expanded)}
        onMouseEnter={(e) => {
          if (linkStatus !== 'broken_fixable' && linkStatus !== 'broken_lost') (e.currentTarget as HTMLElement).style.background = 'var(--bg-row-hover)';
        }}
        onMouseLeave={(e) => {
          if (linkStatus !== 'broken_fixable' && linkStatus !== 'broken_lost') (e.currentTarget as HTMLElement).style.background = 'var(--rowStyle-background, transparent)';
        }}
      >
        {/* icon */}
        <div
          className="w-7 h-7 rounded flex-shrink-0 flex items-center justify-center"
          style={{ color: isLargeFolder ? 'var(--color-warning)' : 'var(--color-primary)' }}
        >
          {isLargeFolder ? <FolderArchive className="w-4 h-4" /> : <AppWindow className="w-4 h-4" />}
        </div>

        {/* name + type + date */}
        <div className="flex-shrink-0" style={{ width: '180px' }}>
          <div className="flex items-center gap-1.5">
            <span className="text-[13px] font-medium truncate" style={{ color: 'var(--text-primary)', maxWidth: '120px' }} title={record.app_name}>
              {record.app_name}
            </span>
            <span className={`badge flex-shrink-0 ${isLargeFolder ? 'text-[var(--color-warning)]' : ''}`}
              style={isLargeFolder ? { background: 'var(--color-warning-light)', color: 'var(--color-warning)' } : undefined}>
              {isLargeFolder ? '文件夹' : '应用'}
            </span>
          </div>
          <p className="text-[11px]" style={{ color: 'var(--text-tertiary)' }}>{formatDate(record.migrated_at)}</p>
        </div>

        {/* path */}
        <div className="flex-1 min-w-0 flex items-center gap-2 text-[11px]" style={{ color: 'var(--text-tertiary)' }}>
          <span className="truncate" style={{ maxWidth: '40%' }} title={record.original_path}>{shortenPath(record.original_path)}</span>
          <ArrowRight className="w-3 h-3 flex-shrink-0" style={{ color: 'var(--text-tertiary)' }} />
          <span className="truncate" style={{ maxWidth: '40%', color: 'var(--color-success)' }} title={record.target_path}>{shortenPath(record.target_path)}</span>
        </div>

        {/* status */}
        <div className="flex-shrink-0 w-5 flex justify-center" title={linkStatus === 'healthy' ? '正常' : linkStatus === 'broken_fixable' ? '用户手动删除或改动，建议清理' : linkStatus === 'broken_lost' ? '数据已丢失，只能清理' : ''}>
          {linkStatus === 'checking' && <Loader2 className="w-3.5 h-3.5 animate-spin" style={{ color: 'var(--text-tertiary)' }} />}
          {linkStatus === 'healthy' && <CheckCircle2 className="w-3.5 h-3.5" style={{ color: 'var(--color-success)' }} />}
          {(linkStatus === 'broken_fixable' || linkStatus === 'broken_lost') && <span title="用户手动绕过 Viap 操作导致，建议清理"><AlertTriangle className="w-3.5 h-3.5" style={{ color: linkStatus === 'broken_lost' ? 'var(--color-danger)' : 'var(--color-warning)' }} /></span>}
        </div>

        {/* size */}
        <span className="text-[11px] tabular-nums flex-shrink-0 w-16 text-right" style={{ color: 'var(--text-secondary)' }}>
          {formatSize(record.size)}
        </span>

        {/* action: 损坏状态 → 清理, 正常 → 恢复 */}
        {(linkStatus === 'broken_fixable' || linkStatus === 'broken_lost') ? (
          <button
            onClick={e => { e.stopPropagation(); onCleanup?.(record.id); }}
            disabled={isRestoring}
            className="btn btn-sm h-6 text-[11px] flex-shrink-0"
            style={{ color: 'var(--color-danger)', borderColor: 'var(--color-danger)' }}
            title="清理残留记录（数据已丢失，无法恢复）"
          >
            {isRestoring ? <Loader2 className="w-3 h-3 animate-spin" /> : <Trash2 className="w-3 h-3" />}
            清理
          </button>
        ) : (
          <button
            onClick={e => { e.stopPropagation(); onRestore(record.id, record.record_type || 'App'); }}
            disabled={isRestoring || linkStatus === 'checking'}
            className="btn btn-sm h-6 text-[11px] flex-shrink-0"
            style={isRestoring ? {
              // 恢复进度直接绘制在按钮底色中，保持历史列表列宽稳定。
              background: `linear-gradient(to right, var(--color-primary-light) 0%, var(--color-primary-light) ${Math.round(restoreProgress ?? 0)}%, transparent ${Math.round(restoreProgress ?? 0)}%)`,
              borderColor: 'var(--color-primary)',
              color: 'var(--color-primary)',
            } : undefined}
            title={linkStatus === 'checking' ? '检测中…' : '恢复'}
          >
            {isRestoring ? <Loader2 className="w-3 h-3 animate-spin" /> : <RotateCcw className="w-3 h-3" />}
            {/* 恢复中展示后端百分比，避免历史页只能看到 loading。 */}
            {isRestoring ? `${Math.round(restoreProgress ?? 0)}%` : '恢复'}
          </button>
        )}

        {/* expand toggle */}
        <div className="flex-shrink-0 w-4">
          {expanded
            ? <ChevronUp className="w-3 h-3" style={{ color: 'var(--text-tertiary)' }} />
            : <ChevronDown className="w-3 h-3" style={{ color: 'var(--text-tertiary)' }} />
          }
        </div>
      </div>

      {/* expand detail panel — 手风琴过渡 */}
      <div
        className="overflow-hidden transition-all duration-200 ease-out"
        style={{
          maxHeight: expanded ? '200px' : '0px',
          opacity: expanded ? 1 : 0,
          borderTop: expanded ? '1px solid var(--border-color)' : '1px solid transparent',
          background: 'var(--bg-row-hover)',
        }}
        onClick={e => e.stopPropagation()}
      >
        <div className="px-5 py-3 grid grid-cols-2 gap-x-8 gap-y-2 text-[11px]">
          <div>
            <span style={{ color: 'var(--text-tertiary)' }}>原始路径</span>
            <p className="break-all mt-0.5" style={{ color: 'var(--text-primary)' }}>{record.original_path}</p>
          </div>
          <div>
            <span style={{ color: 'var(--text-tertiary)' }}>目标路径</span>
            <p className="break-all mt-0.5" style={{ color: 'var(--text-primary)' }}>{record.target_path}</p>
          </div>
          <div>
            <span style={{ color: 'var(--text-tertiary)' }}>迁移时间</span>
            <p style={{ color: 'var(--text-primary)' }}>{formatFullDate(record.migrated_at)}</p>
          </div>
          <div>
            <span style={{ color: 'var(--text-tertiary)' }}>记录 ID</span>
            <p className="break-all text-[10px]" style={{ color: 'var(--text-primary)' }}>{record.id}</p>
          </div>
          <div>
            <span style={{ color: 'var(--text-tertiary)' }}>链接状态</span>
            <p style={{
              color: linkStatus === 'healthy' ? 'var(--color-success)'
                : linkStatus === 'broken_fixable' ? 'var(--color-warning)'
                : linkStatus === 'broken_lost' ? 'var(--color-danger)'
                : 'var(--text-secondary)'
            }}>
              {linkStatus === 'healthy' ? '正常'
                : linkStatus === 'broken_fixable' ? '损坏（用户手动删除或改动，建议清理）'
                : linkStatus === 'broken_lost' ? '严重损坏（数据已丢失，只能清理）'
                : linkStatus === 'checking' ? '检查中'
                : '未知'}
            </p>
          </div>
          <div>
            <span style={{ color: 'var(--text-tertiary)' }}>大小</span>
            <p style={{ color: 'var(--text-primary)' }}>{formatSize(record.size)}</p>
          </div>
        </div>
      </div>
    </div>
  );
}

export default function MigrationHistory({ visible: _visible }: { visible: boolean }) {
  const storeApi = useViapStore;
  const [records, setRecords] = useState<MigrationRecord[]>(() => storeApi.getState().historyRecords);
  const [loading, setLoading] = useState(true);
  const [restoringId, setRestoringId] = useState<string | null>(null);
  const [restoreProgressMap, setRestoreProgressMap] = useState<Record<string, number>>({});
  const [linkStatuses, setLinkStatuses] = useState<Record<string, LinkStatus>>({});
  const { toast, showToast, hideToast } = useToast();

  const [searchQuery, setSearchQuery] = useState('');
  const [filterType, setFilterType] = useState<'all' | 'App' | 'LargeFolder'>('all');
  const [sortBy, setSortBy] = useState<SortBy>('date_desc');

  /** 列头点击排序：同 key 三态切换 asc → desc → 清除（回到 date_desc） */
  function handleColumnSort(key: SortKey) {
    const map: Record<SortKey, [SortBy, SortBy]> = {
      name: ['name_asc', 'name_desc'],
      path: ['path_asc', 'path_desc'],
      size: ['size_desc', 'size_asc'],  // 体积默认降序更符合直觉
    };
    const [asc, desc] = map[key];
    if (sortBy === asc) { setSortBy(desc); }
    else if (sortBy === desc) { setSortBy('date_desc'); }
    else { setSortBy(asc); }
  }
  const [currentPage, setCurrentPage] = useState(1);
  const PAGE_SIZE = 20;

  async function loadHistory() {
    try {
      setLoading(true);
      const history = await invoke<MigrationRecord[]>('get_migration_history');
      setRecords(history);
      storeApi.setState({ historyRecords: history, historyRecordsLoaded: true });

      const initialStatuses: Record<string, LinkStatus> = {};
      const needCheck: MigrationRecord[] = [];
      for (const r of history) {
        const cached = getCachedStatus(r.id);
        if (cached && cached !== 'checking') { initialStatuses[r.id] = cached; }
        else { initialStatuses[r.id] = 'checking'; needCheck.push(r); }
      }
      setLinkStatuses(initialStatuses);

      // concurrent check (max 5)
      async function runWithConcurrency(
        items: MigrationRecord[], limit: number, worker: (r: MigrationRecord) => Promise<void>,
      ) {
        const queue = [...items];
        const active: Promise<void>[] = [];
        async function next() {
          while (queue.length > 0) {
            const item = queue.shift()!;
            const p = worker(item);
            active.push(p);
            p.finally(() => { active.splice(active.indexOf(p), 1); });
            if (active.length >= limit) await Promise.race(active);
          }
          await Promise.all(active);
        }
        await next();
      }

      runWithConcurrency(needCheck, 5, async (record) => {
        try {
          const result = await invoke<{ healthy: boolean; target_exists: boolean }>('check_link_status', { recordId: record.id });
          let status: LinkStatus;
          if (result.healthy) {
            status = 'healthy';
          } else if (result.target_exists) {
            status = 'broken_fixable'; // target 存在，junction 损坏，可修复
          } else {
            status = 'broken_lost';    // target 不存在，数据丢失
          }
          setCachedStatus(record.id, status);
          setLinkStatuses(prev => ({ ...prev, [record.id]: status }));
        } catch {
          setLinkStatuses(prev => ({ ...prev, [record.id]: 'unknown' }));
        }
      });
    } catch (error) {
      console.error('Failed to load history:', error);
      showToast('加载历史记录失败', 'error');
    } finally { setLoading(false); }
  }

  /** 清理损坏的迁移记录（用户手动绕过 Viap 操作导致，直接清理残留） */
  async function handleCleanupBroken(historyId: string) {
    try {
      setRestoringId(historyId);
      const result = await invoke<MigrationResult>('cleanup_broken_record', { historyId });
      if (result.success) {
        showToast(result.message, 'success');
        // 清除健康缓存后重新加载列表
        setCachedStatus(historyId, 'unknown' as LinkStatus);
        await loadHistory();
      } else {
        showToast(result.message || '清理失败', 'error');
      }
    } catch (error) {
      showToast(`清理失败: ${error}`, 'error');
    } finally {
      setRestoringId(null);
    }
  }

  async function handleRestore(historyId: string, _recordType: string) {
    const record = records.find(item => item.id === historyId);
    let unlisten: UnlistenFn | null = null;
    try {
      setRestoringId(historyId);
      setRestoreProgressMap(prev => ({ ...prev, [historyId]: 0 }));
      if (record) {
        try {
          unlisten = await listen<MigrationProgressEvent>('migration-progress', (event) => {
            const data = event.payload;
            // 恢复事件以原路径为 task_id，历史页按记录 ID 更新对应行按钮。
            if (data.task_id.toLowerCase() === record.original_path.toLowerCase()) {
              setRestoreProgressMap(prev => ({ ...prev, [historyId]: data.percent }));
            }
          });
        } catch { /* 监听失败不阻断恢复 */ }
      }
      // 统一使用 restore_app，后端根据 record_type 自动分发恢复逻辑
      // 避免 restore_large_folder 不更新 history 状态导致记录残留的问题
      const result = await invoke<MigrationResult>('restore_app', { historyId });

      if (result.success) {
        showToast('已成功恢复', 'success');
        // 清除该条记录的健康状态缓存，避免缓存过期前显示旧状态
        setCachedStatus(historyId, 'unknown' as LinkStatus);
        await loadHistory();
      } else {
        if (result.message.includes('另一个恢复任务')) {
          showToast('请等待当前恢复任务完成后再操作', 'info');
        } else {
          showToast(result.message, 'error');
        }
      }
    } catch (error) {
      showToast(`恢复失败: ${error}`, 'error');
    } finally {
      if (unlisten) unlisten();
      setRestoreProgressMap(prev => {
        const next = { ...prev };
        delete next[historyId];
        return next;
      });
      setRestoringId(null);
    }
  }

  useEffect(() => {
    // 优先从 Zustand 缓存恢复，避免重复加载
    if (storeApi.getState().historyRecordsLoaded) {
      setRecords(storeApi.getState().historyRecords);
      setLoading(false);
      return;
    }
    loadHistory();
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  const totalSize = records.reduce((sum, r) => sum + r.size, 0);
  const brokenCount = Object.values(linkStatuses).filter(s => s === 'broken_fixable' || s === 'broken_lost').length;

  const filteredRecords = useMemo(() => {
    let result = [...records];
    if (searchQuery.trim()) {
      const q = searchQuery.trim().toLowerCase();
      result = result.filter(r => r.app_name.toLowerCase().includes(q));
    }
    if (filterType !== 'all') {
      result = result.filter(r => (r.record_type || 'App') === filterType);
    }
    result.sort((a, b) => {
      switch (sortBy) {
        case 'date_asc': return a.migrated_at - b.migrated_at;
        case 'size_desc': return b.size - a.size;
        case 'size_asc': return a.size - b.size;
        case 'name_asc': return a.app_name.localeCompare(b.app_name, 'zh-CN');
        case 'name_desc': return b.app_name.localeCompare(a.app_name, 'zh-CN');
        case 'path_asc': return (a.original_path || '').localeCompare(b.original_path || '');
        case 'path_desc': return (b.original_path || '').localeCompare(a.original_path || '');
        default: return b.migrated_at - a.migrated_at;
      }
    });
    return result;
  }, [records, searchQuery, filterType, sortBy]);

  const totalPages = Math.max(1, Math.ceil(filteredRecords.length / PAGE_SIZE));
  const pageRecords = filteredRecords.slice((currentPage - 1) * PAGE_SIZE, currentPage * PAGE_SIZE);

  useEffect(() => { setCurrentPage(1); }, [searchQuery, filterType]);

  return (
    <div className="h-full overflow-hidden flex flex-col" style={{ padding: '12px 16px' }}>
      {/* search / filter / sort + stats + refresh — 固定在顶部，不参与滚动 */}
      <div className="flex items-center gap-2 flex-shrink-0"
        style={{ paddingBottom: '10px', borderBottom: '1px solid var(--border-color)' }}>
          <div className="relative flex-1 max-w-xs">
            <Search className="absolute left-2 top-1/2 -translate-y-1/2 w-3.5 h-3.5" style={{ color: 'var(--text-tertiary)' }} />
            <input
              type="text" placeholder="搜索名称..." value={searchQuery}
              onChange={e => setSearchQuery(e.target.value)}
              className="w-full h-8 pl-7 pr-7 text-[12px] rounded border outline-none transition-colors"
              style={{ background: 'var(--bg-input)', borderColor: 'var(--border-color)', color: 'var(--text-primary)' }}
              onFocus={(e) => { e.currentTarget.style.borderColor = 'var(--color-primary)'; }}
              onBlur={(e) => { e.currentTarget.style.borderColor = 'var(--border-color)'; }}
            />
            {searchQuery && (
              <button
                onClick={() => setSearchQuery('')}
                className="absolute right-1.5 top-1/2 -translate-y-1/2 w-4 h-4 flex items-center justify-center rounded-sm"
                style={{ color: 'var(--text-tertiary)' }}
                onMouseEnter={(e) => { (e.currentTarget as HTMLElement).style.color = 'var(--text-primary)'; }}
                onMouseLeave={(e) => { (e.currentTarget as HTMLElement).style.color = 'var(--text-tertiary)'; }}
              >
                <X className="w-3 h-3" />
              </button>
            )}
          </div>
          <FilterSelect value={filterType} onChange={setFilterType}
            options={[
              { value: 'all' as const, label: '全部类型' },
              { value: 'App' as const, label: '应用' },
              { value: 'LargeFolder' as const, label: '文件夹' },
            ]}
            className="w-[110px]" />
          <FilterSelect value={sortBy} onChange={setSortBy}
            options={[
              { value: 'date_desc' as const, label: '最新优先' },
              { value: 'date_asc' as const, label: '最早优先' },
              { value: 'name_asc' as const, label: '名称 A-Z' },
              { value: 'name_desc' as const, label: '名称 Z-A' },
              { value: 'path_asc' as const, label: '路径 A-Z' },
              { value: 'path_desc' as const, label: '路径 Z-A' },
              { value: 'size_desc' as const, label: '体积最大' },
              { value: 'size_asc' as const, label: '体积最小' },
            ]}
            className="w-[110px]" />
          {/* stats — 原独立行移至此处，节省垂直空间 */}
          {records.length > 0 && (
            <div className="flex items-center gap-3 text-[12px] ml-2">
              <span style={{ color: 'var(--text-secondary)' }}>
                <History className="w-3.5 h-3.5 inline mr-1" style={{ color: 'var(--text-primary)' }} />
                <strong style={{ color: 'var(--text-primary)' }}>{records.length}</strong> 项
              </span>
              <span style={{ color: 'var(--text-secondary)' }}>
                已释放 <strong style={{ color: 'var(--color-success)' }}>{formatSize(totalSize)}</strong>
              </span>
              {brokenCount > 0 && (
                <span style={{ color: 'var(--color-danger)' }}>
                  <AlertTriangle className="w-3.5 h-3.5 inline mr-1" />
                  <strong>{brokenCount}</strong> 个异常
                </span>
              )}
              {filteredRecords.length !== records.length && (
                <span style={{ color: 'var(--text-tertiary)' }}>
                  显示 {filteredRecords.length}/{records.length}
                </span>
              )}
            </div>
          )}
          <button onClick={loadHistory} disabled={loading} className="btn h-8 text-[12px] flex-shrink-0 ml-auto">
            <RefreshCw className={`w-3.5 h-3.5 ${loading ? 'animate-spin' : ''}`} />
          </button>
        </div>

      {/* loading / 空态 */}
      {loading ? (
        <div className="flex-1 flex items-center justify-center">
          <Loader2 className="w-5 h-5 animate-spin" style={{ color: 'var(--color-primary)' }} />
        </div>
      ) : records.length === 0 ? (
        <EmptyState icon={<History />} title="暂无迁移记录" description="迁移应用或文件夹后将在此显示" />
      ) : (
        <>
          {/* column header — 固定，不参与滚动 */}
          <div className="flex items-center gap-3 flex-shrink-0 text-[10px] uppercase tracking-wider"
            style={{ padding: '0 8px', height: '24px', color: 'var(--text-tertiary)', borderBottom: '1px solid var(--border-color-strong)' }}>
            <div className="flex-shrink-0 w-7" />
            <SortHeader label="名称" sortKey="name" sortBy={sortBy} onSort={handleColumnSort}
              style={{ width: '180px', flexShrink: 0 }} />
            <SortHeader label="原路径" sortKey="path" sortBy={sortBy} onSort={handleColumnSort}
              style={{ flex: 1, minWidth: 0 }} />
            <div className="flex-shrink-0 w-5" />
            <SortHeader label="大小" sortKey="size" sortBy={sortBy} onSort={handleColumnSort}
              style={{ width: '64px', textAlign: 'right', flexShrink: 0 }} />
            <span className="flex-shrink-0" style={{ width: '84px' }} />
          </div>

          {/* 列表 — 独立滚动容器 */}
          <div className="flex-1 overflow-y-auto min-h-0">
            {pageRecords.length === 0 ? (
              <EmptyState icon={<Search />} title="无匹配记录" description="尝试调整筛选条件或搜索关键词" />
            ) : (
              <>
                {pageRecords.map(record => (
                  <HistoryRow key={record.id} record={record}
                    onRestore={handleRestore}
                    isRestoring={restoringId === record.id}
                    restoreProgress={restoreProgressMap[record.id]}
                    linkStatus={linkStatuses[record.id] || 'unknown'}
                    onCleanup={handleCleanupBroken} />
                ))}

                {/* pagination */}
                {totalPages > 1 && (
                  <div className="flex items-center justify-center gap-1.5 py-3">
                    <button onClick={() => setCurrentPage(p => Math.max(1, p - 1))} disabled={currentPage === 1}
                      className="btn h-6 text-[11px] px-2">上一页</button>
                    {Array.from({ length: totalPages }, (_, i) => i + 1).map(p => (
                      <button key={p} onClick={() => setCurrentPage(p)}
                        className="h-6 min-w-[24px] text-[11px] rounded border transition-colors"
                        style={{
                          background: p === currentPage ? 'var(--color-primary)' : 'transparent',
                          borderColor: p === currentPage ? 'var(--color-primary)' : 'var(--border-color)',
                          color: p === currentPage ? 'var(--text-inverse)' : 'var(--text-secondary)',
                        }}>
                        {p}
                      </button>
                    ))}
                    <button onClick={() => setCurrentPage(p => Math.min(totalPages, p + 1))} disabled={currentPage === totalPages}
                      className="btn h-6 text-[11px] px-2">下一页</button>
                  </div>
                )}
              </>
            )}
          </div>
        </>
      )}
      {/* Toast 根据通知类型自动选择停留时间，错误提示默认更久。 */}
      <Toast message={toast.message} type={toast.type} visible={toast.visible} duration={toast.duration} onClose={hideToast} />
    </div>
  );
}
