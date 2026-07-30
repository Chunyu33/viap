import { ChevronDown, Check } from 'lucide-react';
import { useEffect, useMemo, useRef, useState } from 'react';
import { createPortal } from 'react-dom';

interface SelectOption<T extends string> {
  value: T;
  label: string;
}

interface FilterSelectProps<T extends string> {
  value: T;
  options: SelectOption<T>[];
  onChange: (value: T) => void;
  className?: string;
  menuClassName?: string;
  /** 卸载等不可变更状态下禁用筛选器，避免用户修改列表视图。 */
  disabled?: boolean;
}

export default function FilterSelect<T extends string>({
  value,
  options,
  onChange,
  className = '',
  menuClassName = '',
  disabled = false,
}: FilterSelectProps<T>) {
  const [open, setOpen] = useState(false);
  const rootRef = useRef<HTMLDivElement>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const [menuPosition, setMenuPosition] = useState<{ top: number; left: number; width: number } | null>(null);

  const selectedLabel = useMemo(() => {
    return options.find((item) => item.value === value)?.label ?? '';
  }, [options, value]);

  useEffect(() => {
    if (!open) return;

    function updatePosition() {
      if (!triggerRef.current) return;
      const rect = triggerRef.current.getBoundingClientRect();
      setMenuPosition({
        top: rect.bottom + 6,
        left: rect.left,
        width: rect.width,
      });
    }

    updatePosition();
    window.addEventListener('resize', updatePosition);
    window.addEventListener('scroll', updatePosition, true);
    return () => {
      window.removeEventListener('resize', updatePosition);
      window.removeEventListener('scroll', updatePosition, true);
    };
  }, [open]);

  useEffect(() => {
    function handleClickOutside(event: MouseEvent) {
      const target = event.target as Node;
      const clickedOutsideTrigger = rootRef.current ? !rootRef.current.contains(target) : true;
      const clickedOutsideMenu = menuRef.current ? !menuRef.current.contains(target) : true;
      if (clickedOutsideTrigger && clickedOutsideMenu) {
        setOpen(false);
      }
    }

    function handleEscape(event: KeyboardEvent) {
      if (event.key === 'Escape') {
        setOpen(false);
      }
    }

    window.addEventListener('mousedown', handleClickOutside);
    window.addEventListener('keydown', handleEscape);
    return () => {
      window.removeEventListener('mousedown', handleClickOutside);
      window.removeEventListener('keydown', handleEscape);
    };
  }, []);

  return (
    <div ref={rootRef} className={`relative ${className}`}>
      <button
        ref={triggerRef}
        type="button"
        onClick={() => setOpen((prev) => !prev)}
        disabled={disabled}
        className="h-8 w-full min-w-[88px] pl-3 pr-8 rounded-md text-[12px] text-left bg-[var(--bg-input)] text-[var(--text-primary)] hover:bg-[var(--bg-hover)] transition-colors border border-[var(--border-color)] focus:outline-none focus:ring-2 focus:ring-[var(--color-primary)]/30 disabled:cursor-default disabled:opacity-60"
        aria-haspopup="listbox"
        aria-expanded={open}
      >
        <span className="block truncate">{selectedLabel}</span>
      </button>

      <ChevronDown
        className={`pointer-events-none absolute right-2 top-1/2 -translate-y-1/2 w-3.5 h-3.5 text-[var(--text-tertiary)] transition-transform ${open ? 'rotate-180' : ''}`}
      />

      {open && menuPosition && createPortal(
        <div
          ref={menuRef}
          className={`absolute top-[calc(100%+6px)] left-0 z-50 overflow-hidden rounded-md bg-[var(--bg-modal)] shadow-[var(--shadow-md)] ring-1 ring-[var(--border-color)] ${menuClassName}`}
          style={{
            position: 'fixed',
            top: menuPosition.top,
            left: menuPosition.left,
            width: menuPosition.width,
            zIndex: 2000,
          }}
          role="listbox"
        >
          {options.map((option) => {
            const selected = option.value === value;
            return (
              <button
                key={option.value}
                type="button"
                onClick={() => {
                  onChange(option.value);
                  setOpen(false);
                }}
                className={`w-full h-8 px-3 text-left text-[12px] inline-flex items-center justify-between transition-colors ${selected
                  ? 'text-[var(--color-primary)] bg-[var(--color-primary-light)]/70'
                  : 'text-[var(--text-primary)] hover:bg-[var(--bg-hover)]'
                }`}
                role="option"
                aria-selected={selected}
              >
                <span>{option.label}</span>
                {selected && <Check className="w-3.5 h-3.5" />}
              </button>
            );
          })}
        </div>
      , document.body)}
    </div>
  );
}
