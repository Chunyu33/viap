// 项目推介弹窗组件
// 展示 LightC 和 BinlockX 两个关联项目的简介与下载信息

import { Code2, Download, Globe } from 'lucide-react';
import Modal from './Modal';
import lightcIcon from '../assets/imgs/lightc.svg';
import binlockxIcon from '../assets/imgs/binlockx.svg';

interface ProjectPromoModalProps {
  isOpen: boolean;
  onClose: () => void;
}

interface ProjectLink {
  label: string;
  url: string;
  /** 决定按钮用哪个图标，缺省按「下载」处理 */
  kind?: 'website' | 'github' | 'download';
}

/** 链接类型 → 图标：官网用地球、GitHub 用代码、下载用箭头 */
const LINK_ICONS: Record<NonNullable<ProjectLink['kind']>, typeof Globe> = {
  website: Globe,
  github: Code2,
  download: Download,
};

interface ProjectInfo {
  name: string;
  summary: string;
  icon: React.ReactNode;
  iconColor: string;
  links: ProjectLink[];
}

const projects: ProjectInfo[] = [
  {
    name: 'LightC',
    summary:
      '一款轻量级、专注 Windows C盘优化的工具。能自动找出并清理 C 盘的临时文件、浏览器缓存、回收站垃圾、社交软件缓存等无用文件，帮你快速释放磁盘空间。同时支持大文件扫描、应用卸载残留清理、右键菜单管理和系统瘦身，操作直观，清理安全。',
    icon: <img src={lightcIcon} className="w-5 h-5 project-promo-icon" alt="LightC" />,
    iconColor: '#F59E0B',
    links: [
      { label: '官网', url: 'https://lightc.app/', kind: 'website' },
      { label: '网盘下载', url: 'https://pan.quark.cn/s/bce8f722bf33', kind: 'download' },
    ],
  },
  {
    name: 'BinlockX',
    summary:
      '一款轻量级，专注本地文件隐私、安全的工具。支持高强度文件加密，即使电脑被他人访问也无法打开你的私密文件。内置隐私空间功能，文件放入后自动隐藏并加密；支持彻底粉碎敏感文件，粉碎后无法恢复。适合保护重要文档、私人照片和工作资料。',
    icon: <img src={binlockxIcon} className="w-5 h-5 project-promo-icon" alt="BinlockX" />,
    iconColor: '#10B981',
    links: [
      { label: '官网', url: 'https://binlockx.evan666.cc/', kind: 'website' },
      { label: '网盘下载', url: 'https://pan.quark.cn/s/4243a5142b29', kind: 'download' },
    ],
  },
];

export default function ProjectPromoModal({ isOpen, onClose }: ProjectPromoModalProps) {
  return (
    <>
      {/* SVG 图标在亮/暗主题下的兼容样式 */}
      <style>{`
        .project-promo-icon {
          object-fit: contain;
          border-radius: 4px;
        }
        /* 暗色背景下 SVG 图标加一层柔和底色，防止深色图形看不清 */
        [data-theme="dark"] .project-promo-icon,
        .dark .project-promo-icon {
          background: rgba(255,255,255,0.08);
          padding: 2px;
          box-sizing: content-box;
          border-radius: 6px;
        }
      `}</style>

      <Modal isOpen={isOpen} onClose={onClose} title="更多实用工具" width={460}>
        <div className="space-y-4">
          {projects.map((proj) => (
            <div
              key={proj.name}
              className="rounded-lg p-4"
              // 面板内的底衬：普通模式下 --bg-inset 就是原来的 --bg-row-hover，
              // 玻璃下换成很淡的一档，避免叠出「贴上去的白块」
              style={{ border: '1px solid var(--border-color)', background: 'var(--bg-inset)' }}
            >
              {/* 项目名 */}
              <div className="flex items-center gap-2 mb-2">
                <div
                  className="w-7 h-7 rounded flex items-center justify-center flex-shrink-0"
                  style={{ color: proj.iconColor }}
                >
                  {proj.icon}
                </div>
                <span className="text-sm font-semibold" style={{ color: 'var(--text-primary)' }}>
                  {proj.name}
                </span>
              </div>

              {/* 简介 */}
              <p className="text-xs mb-3 leading-relaxed" style={{ color: 'var(--text-secondary)' }}>
                {proj.summary}
              </p>

              {/* 下载按钮区 */}
              <div className="flex items-center gap-2">
                {proj.links
                  .filter((link) => link.url)
                  .map((link, index) => {
                    const Icon = LINK_ICONS[link.kind ?? 'download'] ?? Download;
                    return (
                      <a
                        key={index}
                        href={link.url}
                        target="_blank"
                        rel="noopener noreferrer"
                        className="inline-flex items-center gap-1.5 text-xs font-medium no-underline rounded px-3 py-1.5 transition-opacity duration-150"
                        style={{
                          color: 'var(--text-primary)',
                          background: 'var(--bg-toolbar)',
                          border: '1px solid var(--border-color)',
                        }}
                        onMouseEnter={(e) => { (e.currentTarget as HTMLElement).style.opacity = '0.7'; }}
                        onMouseLeave={(e) => { (e.currentTarget as HTMLElement).style.opacity = '1'; }}
                      >
                        <Icon className="w-3 h-3" />
                        {link.label}
                      </a>
                    );
                  })}
                {proj.links.every((link) => !link.url) && (
                  <span className="text-xs" style={{ color: 'var(--text-tertiary)' }}>即将上线，敬请期待</span>
                )}
              </div>
            </div>
          ))}

          <p className="text-[10px] text-center" style={{ color: 'var(--text-tertiary)' }}>
            以上同为我维护的工具，欢迎试试看
          </p>
        </div>
      </Modal>
    </>
  );
}
