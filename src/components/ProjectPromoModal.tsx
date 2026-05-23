// 项目推介弹窗组件
// 展示 LightC 和 BinlockX 两个关联项目的简介与下载信息

import { useEffect, useState, useCallback } from 'react';
import { X, Code2, Download } from 'lucide-react';
import lightcIcon from '../assets/imgs/lightc.svg';
import binlockxIcon from '../assets/imgs/binlockx.svg';

interface ProjectPromoModalProps {
  isOpen: boolean;
  onClose: () => void;
}

interface ProjectLink {
  label: string;
  url: string;
}

interface ProjectInfo {
  name: string;
  summary: string;
  icon: React.ReactNode;
  iconColor: string;
  // iconBg?: string;
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
      { label: 'GitHub', url: 'https://github.com/chunyu33/lightc/releases' },
      { label: '网盘下载', url: 'https://pan.quark.cn/s/bce8f722bf33' },
    ],
  },
  {
    name: 'BinlockX',
    summary:
      '一款轻量级，专注本地文件隐私、安全的工具。支持高强度文件加密，即使电脑被他人访问也无法打开你的私密文件。内置隐私空间功能，文件放入后自动隐藏并加密；支持彻底粉碎敏感文件，粉碎后无法恢复。适合保护重要文档、私人照片和工作资料。',
    icon: <img src={binlockxIcon} className="w-5 h-5 project-promo-icon" alt="BinlockX" />,
    iconColor: '#10B981',
    links: [
      // { label: 'GitHub', url: 'https://github.com/user/binlockx/releases' },
      { label: '网盘下载', url: 'https://pan.quark.cn/s/4243a5142b29' },
    ],
  },
];

export default function ProjectPromoModal({ isOpen, onClose }: ProjectPromoModalProps) {
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
      }, 150);
      return () => clearTimeout(timer);
    }
  }, [isOpen, visible]);

  const handleAnimatedClose = useCallback(() => {
    setLeaving(true);
    setTimeout(() => {
      setVisible(false);
      setLeaving(false);
      onClose();
    }, 150);
  }, [onClose]);

  if (!visible) return null;

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

      <div
        className="fixed inset-0 z-50 grid place-items-center p-4"
        style={{
          animation: leaving ? 'fadeOut 150ms ease-in forwards' : 'fadeIn 150ms ease-out',
        }}
      >
        {/* 半透明遮罩 */}
        <div
          className="absolute inset-0"
          style={{
            background: 'var(--bg-modal-overlay)',
            backdropFilter: 'blur(8px)',
          }}
          onClick={handleAnimatedClose}
        />

        {/* 弹窗主体 */}
        <div
          className={`relative w-full overflow-hidden rounded-xl shadow-lg ${leaving ? 'animate-modal-out' : 'animate-modal-in'}`}
          style={{
            maxWidth: '460px',
            background: 'var(--bg-modal)',
            border: '1px solid var(--border-color)',
          }}
        >
          {/* 标题栏 */}
          <div
            className="flex items-center justify-between px-5 pt-3.5 pb-3"
            style={{ borderBottom: '1px solid var(--border-color)' }}
          >
            <h2 className="text-sm font-semibold" style={{ color: 'var(--text-primary)' }}>
              更多实用工具
            </h2>
            <button onClick={handleAnimatedClose} className="btn btn-ghost btn-icon" aria-label="关闭">
              <X className="h-3.5 w-3.5" />
            </button>
          </div>

          {/* 内容区 */}
          <div className="px-5 py-4 space-y-4" style={{ maxHeight: '60vh', overflowY: 'auto' }}>
            {projects.map((proj) => (
              <div
                key={proj.name}
                className="rounded-lg p-4"
                style={{ border: '1px solid var(--border-color)', background: 'var(--bg-row-hover)' }}
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
                      const isGithub = link.label.includes('GitHub');
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
                          {isGithub ? <Code2 className="w-3 h-3" /> : <Download className="w-3 h-3" />}
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
        </div>
      </div>
    </>
  );
}
