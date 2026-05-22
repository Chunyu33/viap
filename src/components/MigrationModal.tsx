// 迁移进度弹窗组件
// 桌面工具风格：紧凑、克制，与 AppList/CleanupModal 保持一致的视觉语言
// 支持入场/退场过渡动画

import { useEffect, useState, useCallback } from 'react';
import {
  X,
  CheckCircle2,
  AlertTriangle,
  LoaderCircle,
  AlertCircle,
  Ban,
} from 'lucide-react';
import { MigrationStep } from '../types';

interface MigrationModalProps {
  isOpen: boolean;
  step: MigrationStep;
  appName: string;
  message: string;
  lockedProcesses: string[];
  progress: number;
  /** 弹窗标题，默认 "应用迁移" */
  title?: string;
  onCancel?: () => void;
  onForceContinue?: () => void;
  onClose: () => void;
  /** 迁移进行中点击 X 的二次确认回调（由父组件处理 confirm + cancel） */
  onRequestClose: () => void;
}

const stepLabel: Record<MigrationStep, string> = {
  idle: '准备中',
  checking: '检查进程占用',
  counting: '扫描文件列表',
  copying: '复制文件中',
  verifying: '校验完整性',
  linking: '创建目录链接',
  success: '迁移完成',
  error: '迁移失败',
};

export default function MigrationModal({
  isOpen,
  step,
  appName,
  message,
  lockedProcesses,
  progress,
  title = '应用迁移',
  onCancel,
  onForceContinue,
  onClose,
  onRequestClose,
}: MigrationModalProps) {
  // 动画状态管理：visible 控制 DOM 挂载，leaving 控制退场动画
  const [visible, setVisible] = useState(false);
  const [leaving, setLeaving] = useState(false);

  useEffect(() => {
    if (isOpen) {
      setVisible(true);
      setLeaving(false);
    } else if (visible) {
      setLeaving(true);
      const timer = setTimeout(() => {
        setVisible(false);
        setLeaving(false);
      }, 150); // 与 animate-modal-out 时长一致
      return () => clearTimeout(timer);
    }
  }, [isOpen, visible]);

  // 关闭回调包装：先播放退场动画，再通知父组件
  const handleAnimatedClose = useCallback(() => {
    setLeaving(true);
    setTimeout(() => {
      setVisible(false);
      setLeaving(false);
      onClose();
    }, 150);
  }, [onClose]);

  if (!visible) return null;

  const stepText = stepLabel[step] || stepLabel.idle;
  const isLoading = ['checking', 'counting', 'copying', 'verifying', 'linking'].includes(step);
  const canClose = step === 'success' || step === 'error';
  const isSuccess = step === 'success';
  const isError = step === 'error';
  const hasProcessLocks = lockedProcesses.length > 0 && step === 'checking';
  const displayProgress = progress >= 0 && progress <= 100 ? progress : 0;

  return (
    <div
      className="fixed inset-0 z-50 grid place-items-center p-4 animate-fade-in"
      style={{
        animation: leaving
          ? 'fadeOut 150ms ease-in forwards'
          : 'fadeIn 150ms ease-out',
      }}
    >
      {/* 半透明遮罩 */}
      <div
        className="absolute inset-0"
        style={{
          background: 'var(--bg-modal-overlay)',
          backdropFilter: 'blur(8px)',
        }}
        onClick={canClose ? handleAnimatedClose : undefined}
      />

      {/* 弹窗主体 */}
      <div
        className={`relative w-full overflow-hidden rounded-xl shadow-lg ${leaving ? 'animate-modal-out' : 'animate-modal-in'}`}
        style={{
          maxWidth: '400px',
          background: 'var(--bg-modal)',
          border: '1px solid var(--border-color)',
        }}
      >
        {/* 标题栏 — X 始终可见，迁移中点击触发二次确认 */}
        <div
          className="flex items-center justify-between px-5 pt-3.5 pb-3"
          style={{ borderBottom: '1px solid var(--border-color)' }}
        >
          <h2 className="text-sm font-semibold" style={{ color: 'var(--text-primary)' }}>
            {title}
          </h2>
          <button
            onClick={canClose ? handleAnimatedClose : onRequestClose}
            className="btn btn-ghost btn-icon"
            aria-label={canClose ? '关闭弹窗' : '取消迁移'}
          >
            <X className="h-3.5 w-3.5" />
          </button>
        </div>

        {/* 内容区 */}
        <div className="px-5 py-4">
          {/* 目标应用名称 */}
          <p
            className="truncate text-center text-sm font-medium mb-4"
            style={{ color: 'var(--text-secondary)' }}
          >
            {appName}
          </p>

          {/* 当前步骤标识 */}
          <div className="flex items-center justify-center gap-2 mb-3">
            {isLoading && <LoaderCircle className="h-3.5 w-3.5 animate-spin" style={{ color: 'var(--color-primary)' }} />}
            {isSuccess && <CheckCircle2 className="h-3.5 w-3.5" style={{ color: 'var(--color-success)' }} />}
            {isError && <AlertCircle className="h-3.5 w-3.5" style={{ color: 'var(--color-danger)' }} />}
            <span
              className="text-xs font-medium"
              style={{
                color: isSuccess
                  ? 'var(--color-success)'
                  : isError
                  ? 'var(--color-danger)'
                  : 'var(--text-secondary)',
              }}
            >
              {stepText}
            </span>
          </div>

          {/* 进度条 */}
          {isLoading && (
            <div className="mb-3">
              <div
                className="h-1 rounded-full overflow-hidden"
                style={{ background: 'var(--color-gray-200)' }}
              >
                <div
                  className="h-full rounded-full transition-all duration-300 ease-out"
                  style={{
                    width: `${displayProgress}%`,
                    background: 'var(--color-primary)',
                  }}
                />
              </div>
              <p
                className="mt-1.5 text-center"
                style={{ color: 'var(--text-tertiary)', fontSize: 'var(--font-size-xs)' }}
              >
                {displayProgress.toFixed(0)}%
              </p>
            </div>
          )}

          {/* 完成提示 */}
          {isSuccess && (
            <div
              className="rounded-lg px-3 py-2 text-center"
              style={{ background: 'var(--color-success-light)' }}
            >
              <p className="text-xs" style={{ color: 'var(--color-success)' }}>
                应用已从新位置正常运行
              </p>
            </div>
          )}

          {/* 错误详情 */}
          {isError && message && (
            <div className="text-center mb-1">
              <p
                className="text-xs leading-relaxed"
                style={{ color: 'var(--text-secondary)' }}
              >
                {message}
              </p>
            </div>
          )}

          {/* 进程锁警告 */}
          {hasProcessLocks && (
            <div
              className="rounded-lg p-3 mb-1"
              style={{ background: 'var(--color-warning-light)' }}
            >
              <div className="flex items-start gap-2">
                <AlertTriangle className="mt-0.5 h-3.5 w-3.5 flex-shrink-0" style={{ color: 'var(--color-warning)' }} />
                <div>
                  <p className="text-xs font-medium mb-1" style={{ color: 'var(--text-primary)' }}>
                    检测到进程占用
                  </p>
                  <ul className="space-y-0.5">
                    {lockedProcesses.map((proc, i) => (
                      <li key={i} className="text-xs flex items-center gap-1.5" style={{ color: 'var(--text-secondary)' }}>
                        <span className="h-1 w-1 rounded-full flex-shrink-0" style={{ background: 'var(--color-warning)' }} />
                        {proc}
                      </li>
                    ))}
                  </ul>
                </div>
              </div>
            </div>
          )}
        </div>

        {/* 底部按钮 */}
        {(isLoading || hasProcessLocks || canClose) && (
          <div
            className="flex items-center justify-center gap-2 px-5 py-3"
            style={{
              borderTop: '1px solid var(--border-color)',
              background: 'var(--bg-toolbar)',
            }}
          >
            {isLoading && onCancel && (
              <button
                onClick={onCancel}
                className="btn btn-sm inline-flex items-center gap-1.5"
                style={{
                  background: 'var(--color-danger)',
                  color: 'var(--text-inverse)',
                  borderColor: 'var(--color-danger)',
                }}
              >
                <Ban className="w-3.5 h-3.5" />
                取消迁移
              </button>
            )}

            {/* 不提供强制迁移按钮，避免用户误操作 */}
            {/* {hasProcessLocks && onForceContinue && (
              <button
                onClick={onForceContinue}
                className="btn btn-sm"
                style={{
                  background: 'var(--color-primary)',
                  color: 'var(--text-inverse)',
                  borderColor: 'var(--color-primary)',
                }}
              >
                强制继续
              </button>
            )} */}

            {canClose && (
              <button
                onClick={handleAnimatedClose}
                className="btn btn-sm"
                style={{
                  background: 'var(--color-primary)',
                  color: 'var(--text-inverse)',
                  borderColor: 'var(--color-primary)',
                }}
              >
                {isSuccess ? '完成' : '我知道了'}
              </button>
            )}
          </div>
        )}
      </div>
    </div>
  );
}
