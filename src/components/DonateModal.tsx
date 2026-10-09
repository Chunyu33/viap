// 赞赏弹窗组件
// 展示微信/支付宝赞赏码，支持切换

import { useState } from 'react';
import { X, Heart } from 'lucide-react';
import Modal from './Modal';
import WechatQR from '../assets/imgs/r_wechat_qr.jpg';
import AlipayQR from '../assets/imgs/r_alipay_qr.jpg';

interface DonateModalProps {
  isOpen: boolean;
  onClose: () => void;
}

type QRTab = 'wechat' | 'alipay';

const tabs: { key: QRTab; label: string; src: string; color: string }[] = [
  { key: 'wechat', label: '微信', src: WechatQR, color: '#07C160' },
  { key: 'alipay', label: '支付宝', src: AlipayQR, color: '#1677FF' },
];

export default function DonateModal({ isOpen, onClose }: DonateModalProps) {
  const [tab, setTab] = useState<QRTab>('wechat');
  const activeSrc = tabs.find(t => t.key === tab)?.src ?? WechatQR;

  return (
    <Modal
      isOpen={isOpen}
      onClose={onClose}
      width={360}
      // 标题带心形图标，塞不进默认标题栏（默认只接受文字），改用自定义头部
      header={
        <div
          className="flex items-center justify-between flex-shrink-0 px-5 pt-3.5 pb-3"
          style={{ borderBottom: '1px solid var(--border-color)' }}
        >
          <h2 className="text-sm font-semibold flex items-center gap-2" style={{ color: 'var(--text-primary)' }}>
            <Heart className="h-4 w-4" style={{ color: 'var(--color-danger)' }} />
            支持作者
          </h2>
          <button onClick={onClose} className="btn btn-ghost btn-icon" aria-label="关闭">
            <X className="h-3.5 w-3.5" />
          </button>
        </div>
      }
    >
      <p className="text-xs text-center mb-4" style={{ color: 'var(--text-secondary)' }}>
        如果 Viap 帮你省下了磁盘空间，欢迎请我喝杯咖啡 ☕️
      </p>

      {/* 滑块切换 — 微信绿 / 支付宝蓝 */}
      <div className="flex items-center justify-center mb-4">
        <div
          className="relative flex items-center rounded-full text-[12px] font-medium"
          // 滑块轨道用面板内底衬：普通模式下 --bg-inset 就是原来的 --bg-row-hover，
          // 玻璃下换成很淡的一档，选中块（--bg-raised）才浮得起来
          style={{ background: 'var(--bg-inset)', padding: '3px' }}
        >
          {/* 滑动背景块 */}
          <div
            className="absolute top-[3px] h-[28px] rounded-full transition-all duration-250 ease-out"
            style={{
              left: tab === 'wechat' ? '3px' : 'calc(50% + 3px)',
              width: 'calc(50% - 6px)',
              background: tabs.find(t => t.key === tab)?.color ?? '#07C160',
            }}
          />
          {tabs.map(t => (
            <button
              key={t.key}
              onClick={() => setTab(t.key)}
              className="relative z-10 px-5 py-1.5 border-none cursor-pointer rounded-full transition-colors duration-200"
              style={{
                color: tab === t.key ? '#fff' : 'var(--text-tertiary)',
                background: 'transparent',
                minWidth: '80px',
                textAlign: 'center',
              }}
            >
              {t.label}
            </button>
          ))}
        </div>
      </div>

      {/* 赞赏码图片 — 确保扫码尺寸足够 */}
      <div className="flex items-center justify-center">
        <img
          src={activeSrc}
          alt={`${tab} 赞赏码`}
          className="rounded-lg"
          style={{ width: '280px', height: '280px', objectFit: 'contain', background: '#fff' }}
        />
      </div>

      <p className="text-[10px] text-center mt-3" style={{ color: 'var(--text-tertiary)' }}>
        感谢你的支持！
      </p>
    </Modal>
  );
}
