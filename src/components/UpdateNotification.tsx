// 自动更新通知组件
// 应用启动后静默检测更新，发现新版本后展示通知条；
// 下载时显示进度条，安装中提示重启；
// 自动检测失败时静默忽略，不展示错误提示

import { useEffect } from 'react';
import { useUpdater } from '../hooks/useUpdater';
import { ArrowDownToLine, Loader2, X } from 'lucide-react';

export default function UpdateNotification() {
  const {
    status, updateInfo, downloadProgress,
    checkForUpdate, downloadAndInstall, dismiss,
  } = useUpdater();

  // 启动后延迟检测更新，避免影响首屏性能
  useEffect(() => {
    const timer = setTimeout(() => {
      checkForUpdate();
    }, 3000);
    return () => clearTimeout(timer);
  }, [checkForUpdate]);

  // error 也静默忽略：自动检测失败不打扰用户
  if (['idle', 'checking', 'up-to-date', 'error'].includes(status)) {
    return null;
  }

  // 外层容器隔离父级布局穿透，确保横幅独立渲染
  return (
    <div style={{ flexShrink: 0, width: '100%' }}>
      {/* 新版本可用 */}
      {status === 'available' && updateInfo && (
        <div style={{
          display: 'flex', alignItems: 'center', justifyContent: 'space-between',
          padding: '8px 16px', background: 'var(--color-primary-light)',
          borderBottom: '1px solid var(--color-primary)',
          width: '100%',
        }}>
          <div style={{ display: 'flex', alignItems: 'center', gap: '8px', minWidth: 0 }}>
            <ArrowDownToLine style={{ width: 16, height: 16, flexShrink: 0, color: 'var(--color-primary)' }} />
            <div style={{ minWidth: 0 }}>
              <span style={{ fontSize: 'var(--font-size-sm)', fontWeight: 500, color: 'var(--text-primary)' }}>
                发现新版本 v{updateInfo.version}
              </span>
              {updateInfo.notes && (
                <span style={{
                  fontSize: 'var(--font-size-xs)', marginLeft: 8, color: 'var(--text-tertiary)',
                  maxWidth: 320, overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap',
                  display: 'inline-block', verticalAlign: 'bottom',
                }}>
                  {updateInfo.notes}
                </span>
              )}
            </div>
          </div>
          <div style={{ display: 'flex', alignItems: 'center', gap: '8px', flexShrink: 0 }}>
            <button onClick={dismiss} className="btn h-7 text-[11px]">稍后再说</button>
            <button onClick={() => downloadAndInstall()} className="btn btn-primary h-7 text-[11px]">
              立即更新
            </button>
          </div>
        </div>
      )}

      {/* 下载中 */}
      {status === 'downloading' && (
        <div style={{
          display: 'flex', alignItems: 'center', gap: '12px',
          padding: '8px 16px', background: 'var(--color-primary-light)',
          borderBottom: '1px solid var(--color-primary)',
          width: '100%',
        }}>
          <Loader2 style={{ width: 16, height: 16, flexShrink: 0, color: 'var(--color-primary)' }} className="animate-spin" />
          <div style={{ flex: 1, minWidth: 0 }}>
            <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
              <span style={{ fontSize: 'var(--font-size-sm)', color: 'var(--text-primary)' }}>
                正在下载更新
              </span>
              <span style={{ fontSize: 'var(--font-size-xs)', color: 'var(--text-tertiary)' }}>
                {downloadProgress}%
              </span>
            </div>
            {/* 进度条 */}
            <div style={{ marginTop: 4, height: 4, borderRadius: 2, overflow: 'hidden', background: 'var(--bg-row-hover)' }}>
              <div style={{
                height: '100%', borderRadius: 2,
                transition: 'width 300ms ease-out',
                width: `${downloadProgress > 0 ? downloadProgress : 5}%`,
                background: 'var(--color-primary)',
              }} />
            </div>
          </div>
          <button onClick={dismiss} className="btn btn-ghost btn-icon flex-shrink-0" title="取消下载">
            <X style={{ width: 14, height: 14 }} />
          </button>
        </div>
      )}

      {/* 安装中 — relaunch 正常会在安装完成后立即重启，此状态仅短暂出现 */}
      {status === 'installing' && (
        <div style={{
          display: 'flex', alignItems: 'center', gap: '8px',
          padding: '8px 16px', background: 'var(--color-success-light)',
          borderBottom: '1px solid var(--color-success)',
          width: '100%',
          justifyContent: 'space-between',
        }}>
          <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
            <Loader2 style={{ width: 16, height: 16, flexShrink: 0, color: 'var(--color-success)' }} className="animate-spin" />
            <span style={{ fontSize: 'var(--font-size-sm)', color: 'var(--text-primary)' }}>
              正在安装，完成后将自动重启...
            </span>
          </div>
          {/* relaunch 失败后的兜底关闭入口 */}
          <button onClick={dismiss} className="btn btn-ghost btn-icon flex-shrink-0" title="关闭">
            <X style={{ width: 14, height: 14 }} />
          </button>
        </div>
      )}

      {/* 自动检测失败时静默忽略，不展示任何提示 */}
    </div>
  );
}
