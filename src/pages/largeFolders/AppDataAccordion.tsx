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
                background: 'var(--bg-row)',
                // 分类行沿用普通文件夹记录的行高，避免展开区域出现不一致的节奏。
                height: 'var(--row-height)',
                // 仅保留底部分隔线，避免手风琴卡片边框破坏页面的统一列表感。
                borderBottom: '1px solid var(--border-color)',
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
            {expanded && <div>{group.folders.map(renderFolder)}</div>}
          </section>
        );
      })}
    </div>
  );
}
