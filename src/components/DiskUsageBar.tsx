// 全局磁盘状态胶囊组件
// 以状态胶囊 + Popover 的方式展示磁盘信息

import { useEffect, useMemo, useRef, useState, CSSProperties } from 'react';
import { HardDrive, PieChart, RefreshCw } from 'lucide-react';
import { DiskUsage } from '../types';

interface DiskUsageBarProps {
  disks: DiskUsage[];
  loading: boolean;
  refreshing?: boolean;
  onRefresh?: () => void;
}

// 格式化字节数为简短格式
function formatBytes(bytes: number): string {
  if (bytes === 0) return '0 B';
  const k = 1024;
  const sizes = ['B', 'KB', 'MB', 'GB', 'TB'];
  const i = Math.floor(Math.log(bytes) / Math.log(k));
  return parseFloat((bytes / Math.pow(k, i)).toFixed(1)) + ' ' + sizes[i];
}

// 根据使用率返回颜色
function getUsageColor(percent: number): string {
  if (percent >= 90) return 'var(--color-danger)';
  if (percent >= 70) return 'var(--color-warning)';
  return 'var(--color-primary)';
}

function getDisplayName(mountPoint: string): string {
  return mountPoint.replace(':\\', '');
}

// Popover 内部磁盘行
const DiskRow = ({ disk }: { disk: DiskUsage }) => {
  const usageColor = useMemo(() => getUsageColor(disk.usage_percent), [disk.usage_percent]);
  const displayName = getDisplayName(disk.mount_point);
  const capacityText = `${formatBytes(disk.free_space)} / ${formatBytes(disk.total_space)}`;
  
  return (
    <div
      style={{
        display: 'flex',
        alignItems: 'center',
        gap: '10px',
        padding: '10px 12px',
        background: 'var(--bg-hover)',
        borderRadius: 'var(--radius-sm)',
        border: '1px solid var(--border-color)',
      }}
    >
      <div
        style={{
          width: '16px',
          height: '16px',
          borderRadius: 'var(--radius-sm)',
          background: disk.is_system ? 'var(--color-primary-light)' : 'var(--color-gray-200)',
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'center',
          flexShrink: 0,
        }}
      >
        <HardDrive 
          style={{ 
            width: '9px', 
            height: '9px', 
            color: disk.is_system ? 'var(--color-primary)' : 'var(--text-tertiary)' 
          }} 
        />
      </div>

      <span style={{ fontSize: '12px', fontWeight: 600, color: 'var(--text-primary)', width: '30px', flexShrink: 0 }}>
        {displayName}:
      </span>

      <div
        style={{
          width: '130px',
          flexShrink: 0,
          fontSize: '11px',
          color: 'var(--text-secondary)',
          whiteSpace: 'nowrap',
        }}
        title={`可用/总计 ${capacityText}`}
      >
        {capacityText}
      </div>

      <div
        style={{
          flex: 1,
          minWidth: '120px',
          height: '4px',
          background: 'var(--color-gray-300)',
          borderRadius: '999px',
          overflow: 'hidden',
        }}
      >
        <div
          style={{
            width: `${Math.min(disk.usage_percent, 100)}%`,
            height: '100%',
            background: usageColor,
            borderRadius: '999px',
            transition: 'width 0.2s ease',
          }}
        />
      </div>

      <span style={{ fontSize: '11px', fontWeight: 700, color: usageColor, width: '34px', textAlign: 'right', flexShrink: 0 }}>
        {disk.usage_percent.toFixed(0)}%
      </span>
    </div>
  );
};

const DiskRowSkeleton = () => (
  <div
    className="animate-pulse"
    style={{
      display: 'flex',
      alignItems: 'center',
      gap: '10px',
      padding: '10px 12px',
      background: 'var(--bg-hover)',
      borderRadius: 'var(--radius-sm)',
      border: '1px solid var(--border-color)',
    }}
  >
    <div style={{ width: '16px', height: '16px', borderRadius: 'var(--radius-sm)', background: 'var(--color-gray-200)', flexShrink: 0 }} />
    <div style={{ width: '30px', height: '11px', borderRadius: '4px', background: 'var(--color-gray-200)', flexShrink: 0 }} />
    <div style={{ width: '130px', height: '11px', borderRadius: '4px', background: 'var(--color-gray-200)', flexShrink: 0 }} />
    <div style={{ flex: 1, height: '4px', borderRadius: '999px', background: 'var(--color-gray-200)' }} />
    <div style={{ width: '30px', height: '11px', borderRadius: '4px', background: 'var(--color-gray-200)', flexShrink: 0 }} />
  </div>
);

export default function DiskUsageBar({ disks, loading, refreshing = false, onRefresh }: DiskUsageBarProps) {
  const [open, setOpen] = useState(false);
  const [capsuleHover, setCapsuleHover] = useState(false);
  const containerRef = useRef<HTMLDivElement>(null);

  const primaryDisk = useMemo(() => {
    if (!disks || disks.length === 0) return null;
    return disks.find((disk) => disk.mount_point.toUpperCase().startsWith('C')) || disks[0];
  }, [disks]);

  const summaryText = useMemo(() => {
    if (loading) return '加载中';
    if (!primaryDisk) return '无数据';
    const name = getDisplayName(primaryDisk.mount_point);
    return `${name}: ${primaryDisk.usage_percent.toFixed(0)}%`;
  }, [loading, primaryDisk]);

  useEffect(() => {
    function handleClickOutside(event: MouseEvent) {
      if (!containerRef.current) return;
      if (event.target instanceof Node && !containerRef.current.contains(event.target)) {
        setOpen(false);
      }
    }

    document.addEventListener('mousedown', handleClickOutside);
    return () => document.removeEventListener('mousedown', handleClickOutside);
  }, []);

  return (
    <div
      ref={containerRef}
      style={{ position: 'relative', display: 'flex', alignItems: 'center', gap: '6px' }}
    >
      <button
        onClick={() => setOpen((prev) => !prev)}
        onMouseEnter={() => setCapsuleHover(true)}
        onMouseLeave={() => setCapsuleHover(false)}
        style={{
          display: 'flex',
          alignItems: 'center',
          gap: '6px',
          height: '32px',
          padding: '0 12px',
          borderRadius: '999px',
          border: '1px solid var(--border-color)',
          background: capsuleHover ? 'var(--bg-hover)' : 'var(--bg-content)',
          color: 'var(--text-primary)',
          cursor: 'pointer',
          fontSize: '12px',
          fontWeight: 600,
          transition: 'background 0.15s ease, border-color 0.15s ease',
        } as CSSProperties}
        title="查看磁盘状态"
      >
        <PieChart style={{ width: '13px', height: '13px', color: 'var(--color-primary)' }} />
        <span>{summaryText}</span>
      </button>

      {open && (
        <div
          className="glass-panel"
          style={{
            position: 'absolute',
            right: 0,
            top: 'calc(100% + 8px)',
            width: '500px',
            maxWidth: 'min(500px, calc(100vw - 40px))',
            padding: '12px',
            borderRadius: 'var(--radius-md)',
            border: '1px solid var(--border-color)',
            background: 'var(--bg-modal)',
            boxShadow: 'var(--shadow-lg)',
            zIndex: 1200,
          }}
        >
          <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', marginBottom: '10px' }}>
            <span style={{ fontSize: '12px', fontWeight: 700, color: 'var(--text-primary)' }}>磁盘状态</span>
            <div className="flex items-center gap-2">
              <span style={{ fontSize: '11px', color: 'var(--text-secondary)' }}>{disks?.length ?? 0} 个磁盘</span>
              <button
                onClick={(e) => { e.stopPropagation(); onRefresh?.(); }}
                disabled={!onRefresh || refreshing}
                className="btn btn-ghost btn-icon w-6 h-6"
                title="刷新磁盘状态"
              >
                <RefreshCw style={{ width: '12px', height: '12px' }} className={refreshing ? 'animate-spin' : ''} />
              </button>
            </div>
          </div>

          <div style={{ display: 'flex', flexDirection: 'column', gap: '6px' }}>
            {loading && [1, 2, 3].map((index) => <DiskRowSkeleton key={index} />)}
            {!loading && (!disks || disks.length === 0) && (
              <div
                style={{
                  padding: '10px',
                  borderRadius: 'var(--radius-sm)',
                  border: '1px solid var(--border-color)',
                  background: 'var(--bg-hover)',
                  fontSize: '12px',
                  color: 'var(--text-tertiary)',
                  textAlign: 'center',
                }}
              >
                无法获取磁盘信息
              </div>
            )}
            {!loading && disks && disks.map((disk) => (
              <DiskRow key={disk.mount_point} disk={disk} />
            ))}
          </div>
        </div>
      )}
    </div>
  );
}
