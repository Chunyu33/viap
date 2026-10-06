import { useMemo, useState, type ReactNode } from 'react';
import { ChevronDown } from 'lucide-react';
import type { LargeFolder } from '../../types';
import { groupAppDataFolders } from './appDataGroups';

function formatSize(bytes: number): string {
  if (bytes === 0) return '--';
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  const unitIndex = Math.min(Math.floor(Math.log(bytes) / Math.log(1024)), units.length - 1);
  return `${parseFloat((bytes / Math.pow(1024, unitIndex)).toFixed(2))} ${units[unitIndex]}`;
}

interface AppDataAccordionProps {
  folders: LargeFolder[];
  renderFolder: (folder: LargeFolder) => ReactNode;
}

/** 应用数据手风琴只管理展示状态，不在展开分类时触发目录扫描。 */
export default function AppDataAccordion({ folders, renderFolder }: AppDataAccordionProps) {
  const groups = useMemo(() => groupAppDataFolders(folders), [folders]);
  // 默认全部折叠，避免进入页面时同时渲染大量应用数据行。
  const [expandedGroups, setExpandedGroups] = useState<Set<string>>(() => new Set());

  const toggleGroup = (groupId: string) => {
    setExpandedGroups((current) => {
      const next = new Set(current);
      if (next.has(groupId)) next.delete(groupId);
      else next.add(groupId);
      return next;
    });
  };

  return (
    <div>
      {groups.map((group) => {
        const expanded = expandedGroups.has(group.id);
        const totalSize = group.folders.reduce((total, folder) => total + folder.size, 0);
        return (
          <section key={group.id}>
            <button
              type="button"
              onClick={() => toggleGroup(group.id)}
              className="flex items-center justify-between w-full px-2.5 text-left"
              style={{
                color: 'var(--text-primary)',
                // 分类行就长在列表卡片里，底色必须比卡片只高一点点。
                // 普通模式下 --bg-group 等于 --bg-content —— 与普通行同色，看不出色块；
                // 液态玻璃下卡片本身是半透明的，原来那层 60% 白叠上去会变成 84%，
                // 整行糊成一块实色，所以由 --bg-group 单独给一个很低的透明度。
                background: 'var(--bg-group)',
                // 分类行沿用普通文件夹记录的行高，避免展开区域出现不一致的节奏。
                height: 'var(--row-height)',
                // 仅保留底部分隔线，避免手风琴卡片边框破坏页面的统一列表感；
                // 液态玻璃外观下这条线会被移除（token 归零），由行高与悬浮高亮区分。
                borderBottom: '1px solid var(--border-color-row)',
              }}
              aria-expanded={expanded}
            >
              <span className="flex items-center gap-2 min-w-0">
                <ChevronDown className={`w-3.5 h-3.5 flex-shrink-0 transition-transform ${expanded ? '' : '-rotate-90'}`} />
                <span className="text-[12px] font-medium truncate">{group.title}</span>
                <span className="badge" style={{ color: 'var(--text-tertiary)' }}>{group.folders.length}</span>
              </span>
              <span className="text-[11px] tabular-nums flex-shrink-0" style={{ color: 'var(--text-secondary)' }}>
                {formatSize(totalSize)}
              </span>
            </button>
            <div
              className="grid transition-[grid-template-rows,opacity] duration-200 ease-in-out"
              style={{
                // 保持内容挂载，通过 grid 行高过渡实现真正的手风琴动画。
                gridTemplateRows: expanded ? '1fr' : '0fr',
                opacity: expanded ? 1 : 0,
                pointerEvents: expanded ? 'auto' : 'none',
              }}
              aria-hidden={!expanded}
            >
              <div className="min-h-0 overflow-hidden">
                {group.folders.map(renderFolder)}
              </div>
            </div>
          </section>
        );
      })}
    </div>
  );
}
