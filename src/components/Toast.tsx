// Toast 通知组件
// 使用全局 CSS 变量保持与主题配色一致

import { useCallback, useEffect, useRef, useState } from 'react';
import { CheckCircle2, XCircle, Info, X } from 'lucide-react';

export type ToastType = 'success' | 'error' | 'info';

interface ToastProps {
  message: string;
  type: ToastType;
  visible: boolean;
  onClose: () => void;
  /** 允许个别场景覆盖默认展示时长；不传时按通知级别自动选择。 */
  duration?: number;
}

const typeColors = {
  success: {
    bg: 'var(--color-success-light)',
    border: 'var(--color-success)',
    text: 'var(--color-success)',
  },
  error: {
    bg: 'var(--color-danger-light)',
    border: 'var(--color-danger)',
    text: 'var(--color-danger)',
  },
  info: {
    bg: 'var(--color-primary-light)',
    border: 'var(--color-primary)',
    text: 'var(--color-primary)',
  },
} as const;

const typeIcon = {
  success: CheckCircle2,
  error: XCircle,
  info: Info,
};

const defaultDurationByType: Record<ToastType, number> = {
  success: 3000,
  info: 4000,
  // 错误信息通常更长且需要用户读完，默认停留 8 秒减少误消失。
  error: 8000,
};

export default function Toast({ message, type, visible, onClose, duration }: ToastProps) {
  const [isLeaving, setIsLeaving] = useState(false);
  const displayDuration = duration ?? defaultDurationByType[type];
  const autoCloseTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const animationTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null);
  const startedAtRef = useRef(0);
  const remainingDurationRef = useRef(0);
  const pausedRef = useRef(false);

  const clearAutoCloseTimer = useCallback(() => {
    if (autoCloseTimerRef.current) {
      clearTimeout(autoCloseTimerRef.current);
      autoCloseTimerRef.current = null;
    }
  }, []);

  const clearAnimationTimer = useCallback(() => {
    if (animationTimerRef.current) {
      clearTimeout(animationTimerRef.current);
      animationTimerRef.current = null;
    }
  }, []);

  const closeWithAnimation = useCallback(() => {
    clearAutoCloseTimer();
    setIsLeaving(true);
    clearAnimationTimer();
    animationTimerRef.current = setTimeout(onClose, 200);
  }, [clearAnimationTimer, clearAutoCloseTimer, onClose]);

  const startAutoCloseTimer = useCallback((nextDuration: number) => {
    if (!visible || nextDuration <= 0) return;
    clearAutoCloseTimer();
    startedAtRef.current = Date.now();
    remainingDurationRef.current = nextDuration;
    autoCloseTimerRef.current = setTimeout(closeWithAnimation, nextDuration);
  }, [clearAutoCloseTimer, closeWithAnimation, visible]);

  useEffect(() => {
    if (visible) {
      // 每次显示新 Toast 时重置计时状态，避免上一条的暂停剩余时间影响下一条。
      setIsLeaving(false);
      pausedRef.current = false;
      startAutoCloseTimer(displayDuration);
    }

    return () => {
      clearAutoCloseTimer();
      clearAnimationTimer();
    };
  }, [clearAnimationTimer, clearAutoCloseTimer, displayDuration, startAutoCloseTimer, visible]);

  const handleMouseEnter = useCallback(() => {
    if (!visible || isLeaving || displayDuration <= 0 || !autoCloseTimerRef.current) return;
    // Hover 时保留剩余时间，方便用户阅读较长错误内容，不让 Toast 在鼠标下突然消失。
    const elapsed = Date.now() - startedAtRef.current;
    remainingDurationRef.current = Math.max(0, remainingDurationRef.current - elapsed);
    pausedRef.current = true;
    clearAutoCloseTimer();
  }, [clearAutoCloseTimer, displayDuration, isLeaving, visible]);

  const handleMouseLeave = useCallback(() => {
    if (!visible || isLeaving || displayDuration <= 0 || !pausedRef.current) return;
    pausedRef.current = false;
    startAutoCloseTimer(remainingDurationRef.current);
  }, [displayDuration, isLeaving, startAutoCloseTimer, visible]);

  if (!visible) return null;

  const colors = typeColors[type];
  const Icon = typeIcon[type];

  return (
    <div className="fixed top-4 right-4 z-[100] pointer-events-none">
      <div
        className={`pointer-events-auto flex items-center gap-3 px-4 py-3 rounded-md border shadow-md transition-all duration-200 ease-out ${
          isLeaving ? 'opacity-0 translate-x-4' : 'opacity-100 translate-x-0'
        }`}
        onMouseEnter={handleMouseEnter}
        onMouseLeave={handleMouseLeave}
        style={{
          background: colors.bg,
          borderColor: colors.border,
        }}
      >
        <Icon className="w-5 h-5 flex-shrink-0" style={{ color: colors.text }} />
        <p className="text-sm font-medium max-w-[280px] whitespace-pre-line break-words" style={{ color: colors.text }}>{message}</p>
        <button
          onClick={closeWithAnimation}
          className="w-6 h-6 flex items-center justify-center rounded-md transition-colors"
          style={{ color: colors.text }}
        >
          <X className="w-4 h-4" />
        </button>
      </div>
    </div>
  );
}

export function useToast() {
  const [toast, setToast] = useState<{
    message: string;
    type: ToastType;
    visible: boolean;
    duration?: number;
  }>({ message: '', type: 'info', visible: false });

  const showToast = useCallback((message: string, type: ToastType = 'info', duration?: number) => {
    // duration 只在特殊场景传入；常规页面按类型使用 Toast 默认时长，避免调用处重复配置。
    setToast({ message, type, visible: true, duration });
  }, []);

  const hideToast = useCallback(() => {
    setToast(prev => ({ ...prev, visible: false }));
  }, []);

  return { toast, showToast, hideToast };
}
