// 通用右键菜单
//
// 通过 portal 渲染到 body：列表本身在滚动容器里，若就地渲染会被 overflow 裁剪。
// 关闭时机：点击别处、Esc、滚动、窗口尺寸变化——与系统右键菜单的行为保持一致。

import { useEffect, useLayoutEffect, useRef, useState } from 'react';
import { createPortal } from 'react-dom';
import type { ReactNode } from 'react';

export interface ContextMenuItem {
  key: string;
  label: string;
  icon?: ReactNode;
  /** 危险操作：红色显示，提示不可逆 */
  danger?: boolean;
  disabled?: boolean;
  onSelect: () => void;
}

interface ContextMenuProps {
  /** 打开位置（视口坐标） */
  x: number;
  y: number;
  items: ContextMenuItem[];
  onClose: () => void;
}

/** 菜单与视口边缘的安全距离 */
const VIEWPORT_MARGIN = 8;

export default function ContextMenu({ x, y, items, onClose }: ContextMenuProps) {
  const menuRef = useRef<HTMLDivElement>(null);
  const [position, setPosition] = useState({ left: x, top: y });

  // 菜单尺寸测量后再决定是否翻转，避免贴边时被裁掉
  useLayoutEffect(() => {
    const menu = menuRef.current;
    if (!menu) return;
    const { width, height } = menu.getBoundingClientRect();
    const maxLeft = window.innerWidth - width - VIEWPORT_MARGIN;
    const maxTop = window.innerHeight - height - VIEWPORT_MARGIN;
    setPosition({
      left: Math.max(VIEWPORT_MARGIN, Math.min(x, maxLeft)),
      top: Math.max(VIEWPORT_MARGIN, Math.min(y, maxTop)),
    });
  }, [x, y]);

  useEffect(() => {
    function handlePointerDown(event: MouseEvent) {
      if (menuRef.current && !menuRef.current.contains(event.target as Node)) {
        onClose();
      }
    }
    function handleKeyDown(event: KeyboardEvent) {
      if (event.key === 'Escape') onClose();
    }

    window.addEventListener('mousedown', handlePointerDown);
    window.addEventListener('keydown', handleKeyDown);
    // 滚动或窗口变化后菜单位置就失去意义，直接关闭
    window.addEventListener('scroll', onClose, true);
    window.addEventListener('resize', onClose);
    return () => {
      window.removeEventListener('mousedown', handlePointerDown);
      window.removeEventListener('keydown', handleKeyDown);
      window.removeEventListener('scroll', onClose, true);
      window.removeEventListener('resize', onClose);
    };
  }, [onClose]);

  return createPortal(
    <div
      ref={menuRef}
      className="fixed z-[1200] min-w-[140px] overflow-hidden rounded-md py-1"
      style={{
        left: position.left,
        top: position.top,
        background: 'var(--bg-modal)',
        border: '1px solid var(--border-color)',
        boxShadow: 'var(--shadow-lg)',
      }}
      role="menu"
    >
      {items.map((item) => (
        <button
          key={item.key}
          type="button"
          role="menuitem"
          disabled={item.disabled}
          onClick={() => {
            onClose();
            item.onSelect();
          }}
          className="flex w-full items-center gap-2 px-3 py-1.5 text-left text-[12px] transition-colors disabled:cursor-default disabled:opacity-40"
          style={{ color: item.danger ? 'var(--color-danger)' : 'var(--text-primary)' }}
          onMouseEnter={(event) => {
            if (item.disabled) return;
            (event.currentTarget as HTMLElement).style.background = 'var(--bg-row-hover)';
          }}
          onMouseLeave={(event) => {
            (event.currentTarget as HTMLElement).style.background = 'transparent';
          }}
        >
          {item.icon}
          {item.label}
        </button>
      ))}
    </div>,
    document.body,
  );
}
