// 通用弹窗组件 — 全项目弹窗的统一外壳
//
// 负责遮罩（含点击关闭策略）、面板（圆角/底色/描边/阴影）、进出场动画、
// 标题栏与底部操作区。各业务弹窗只提供自己的内容，不再各自复制一套外壳
// （此前 8 个弹窗各写了一遍遮罩 + 面板 + 150ms 动画状态机）。
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
  /** 标题栏文字。需要副标题或图标时改用 header 插槽 */
  title?: string;
  /** 自定义标题栏内容，替代默认的「标题 + 关闭按钮」 */
  header?: ReactNode;
  children: ReactNode;
  /** 底部操作区：固定在面板底部，不随内容滚动 */
  footer?: ReactNode;
  /** 面板宽度。数字按 px 处理并自动留出窗口边距，也可直接传 CSS 表达式 */
  width?: number | string;
  /** 面板最大高度 */
  maxHeight?: string;
  /** 点遮罩是否关闭，默认 true。需要用户显式操作的弹窗（残留清理、目录选择）传 false */
  closeOnOverlay?: boolean;
  /**
   * 内容容器的类名，会**完整替换**默认值（默认：可滚动 + 标准内边距）。
   * 需要自己控制滚动与布局的弹窗（例如内容区滚动、底栏固定）传自己的组合。
   */
  bodyClassName?: string;
}

export default function Modal({
  isOpen,
  onClose,
  title,
  header,
  children,
  footer,
  width = 640,
  maxHeight = 'min(640px, calc(100vh - 80px))',
  closeOnOverlay = true,
  bodyClassName = 'flex-1 overflow-y-auto px-5 py-4',
}: ModalProps) {
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

  // Escape 关闭：统一到外壳，避免只有个别弹窗能按 Esc 退出。
  // 与遮罩点击共用 closeOnOverlay 开关 —— 「必须显式操作」的弹窗两者都不该生效。
  useEffect(() => {
    if (!isOpen || !mounted || !closeOnOverlay) return;
    const handler = (e: KeyboardEvent) => {
      if (e.key === 'Escape') handleClose();
    };
    document.addEventListener('keydown', handler);
    return () => document.removeEventListener('keydown', handler);
  }, [isOpen, mounted, closeOnOverlay, handleClose]);

  if (!mounted) return null;

  const overlayStyle: React.CSSProperties = {
    background: 'var(--bg-modal-overlay)',
    // 遮罩自带模糊。原来的自建弹窗都是这么做的，统一到这里后所有弹窗口径一致。
    backdropFilter: 'blur(8px)',
    opacity: show ? 1 : 0,
    transition: 'opacity 180ms ease-out',
  };

  const panelStyle: React.CSSProperties = {
    width: typeof width === 'number' ? `min(${width}px, calc(100vw - 48px))` : width,
    maxHeight,
    // 底色与投影走语义 token：普通模式下分别等于 --bg-modal / --shadow-lg（外观不变），
    // 液态玻璃外观下换成更透的底色与带内描边高光的投影，由 .glass-panel 统一接管。
    // 这里必须写 token 名而不是直接写 --bg-modal —— 内联样式优先级高于 .glass-panel 规则，
    // 沿用 --bg-modal 会把玻璃底色顶回不透明。
    background: 'var(--bg-modal-panel)',
    border: '1px solid var(--border-color)',
    boxShadow: 'var(--panel-shadow)',
    transform: show ? 'scale(1)' : 'scale(0.96)',
    opacity: show ? 1 : 0,
    transition: 'transform 180ms ease-out, opacity 180ms ease-out',
  };

  return (
    <div
      // z-index 必须高于标题栏：液态玻璃下标题栏被提到 1300（否则会被内容区盖住），
      // 弹窗若停在 1000 会出现「遮罩压不住标题栏」——标题栏亮着、其余部分变暗。
      className="fixed inset-0 z-[1400] flex items-center justify-center"
      style={overlayStyle}
      onClick={closeOnOverlay ? handleClose : undefined}
    >
      <div
        className="rounded-xl flex flex-col overflow-hidden glass-panel"
        style={panelStyle}
        onClick={(e) => e.stopPropagation()}
      >
        {/* 标题栏：给了 header 就用它；否则在有 title 时渲染默认样式；
            两者都没有则不渲染 —— 这类弹窗（标题本身就是内容的一部分）
            需要自行在内容里提供关闭入口。 */}
        {header ?? (title !== undefined ? (
          <div
            className="flex items-center justify-between flex-shrink-0 px-5 py-3"
            style={{ borderBottom: '1px solid var(--border-color)' }}
          >
            <h2 className="text-[14px] font-semibold" style={{ color: 'var(--text-primary)' }}>
              {title}
            </h2>
            <button
              onClick={handleClose}
              className="btn btn-ghost btn-icon w-7 h-7"
              aria-label="关闭"
            >
              <X className="w-4 h-4" />
            </button>
          </div>
        ) : null)}

        <div className={bodyClassName}>
          {children}
        </div>

        {/* 底部操作区由外层包一层，保证它始终不参与压缩、不随内容滚动 */}
        {footer && <div className="flex-shrink-0">{footer}</div>}
      </div>
    </div>
  );
}
