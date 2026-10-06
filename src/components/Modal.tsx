// 通用弹窗组件 — 桌面工具风格
// 支持打开/关闭过渡动画
//
// 动画原理（React 18 兼容）：
// 1. 打开：setTimeout 确保起始样式先被浏览器绘制，然后再切换到最终样式触发 CSS transition
// 2. 关闭：同样用 setTimeout 等待关闭动画播完再卸载 DOM
// 注意：用 setTimeout 而非 requestAnimationFrame，因为 React 18 会批处理 RAF 中的 setState，
//       导致起始样式和最终样式合并为一次渲染，动画无法触发。

import { ReactNode, useState, useEffect, useCallback } from 'react';
import { X } from 'lucide-react';

interface ModalProps {
  isOpen: boolean;
  onClose: () => void;
  title?: string;
  children: ReactNode;
  width?: number;
}

export default function Modal({ isOpen, onClose, title, children, width = 640 }: ModalProps) {
  const [mounted, setMounted] = useState(false);
  const [show, setShow] = useState(false);

  useEffect(() => {
    if (isOpen) {
      setMounted(true);
      // setTimeout 不参与 React 18 批处理，确保两帧独立渲染
      const t = setTimeout(() => setShow(true), 10);
      return () => clearTimeout(t);
    } else if (mounted) {
      setShow(false);
      const t = setTimeout(() => {
        setMounted(false);
        onClose();
      }, 180);
      return () => clearTimeout(t);
    }
  }, [isOpen]); // eslint-disable-line react-hooks/exhaustive-deps

  const handleClose = useCallback(() => {
    setShow(false);
    setTimeout(() => {
      setMounted(false);
      onClose();
    }, 180);
  }, [onClose]);

  if (!mounted) return null;

  const overlayStyle: React.CSSProperties = {
    background: 'var(--bg-modal-overlay)',
    opacity: show ? 1 : 0,
    transition: 'opacity 180ms ease-out',
  };

  const panelStyle: React.CSSProperties = {
    width: `min(${width}px, calc(100vw - 48px))`,
    maxHeight: 'min(640px, calc(100vh - 80px))',
    background: 'var(--bg-modal)',
    border: '1px solid var(--border-color)',
    boxShadow: 'var(--shadow-lg)',
    transform: show ? 'scale(1)' : 'scale(0.96)',
    opacity: show ? 1 : 0,
    transition: 'transform 180ms ease-out, opacity 180ms ease-out',
  };

  return (
    <div
      className="fixed inset-0 z-[1000] flex items-center justify-center"
      style={overlayStyle}
      onClick={handleClose}
    >
      <div
        className="rounded-lg flex flex-col overflow-hidden glass-panel"
        style={panelStyle}
        onClick={(e) => e.stopPropagation()}
      >
        <div
          className="flex items-center justify-between flex-shrink-0 px-5 py-3"
          style={{ borderBottom: '1px solid var(--border-color)' }}
        >
          <h2 className="text-[14px] font-semibold" style={{ color: 'var(--text-primary)' }}>
            {title ?? ''}
          </h2>
          <button
            onClick={handleClose}
            className="btn btn-ghost btn-icon w-7 h-7"
            aria-label="关闭"
          >
            <X className="w-4 h-4" />
          </button>
        </div>

        <div className="flex-1 overflow-y-auto px-5 py-4">
          {children}
        </div>
      </div>
    </div>
  );
}
