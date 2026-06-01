// 用户手册组件
// 内容按逻辑顺序编排：概述 → 迁移原理 → 强力卸载 → 其他功能 → 数据安全 → 使用协议

import { color } from 'framer-motion';
import Modal from './Modal';

interface UserManualProps {
  isOpen: boolean;
  onClose: () => void;
}

function Section({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <section style={{ paddingBottom: '20px', marginBottom: '20px', borderBottom: '1px solid var(--border-color)' }}>
      <h3 className="text-[13px] font-semibold mb-3" style={{ color: 'var(--text-primary)' }}>{title}</h3>
      <div className="text-[12px] leading-relaxed" style={{ color: 'var(--text-secondary)' }}>
        {children}
      </div>
    </section>
  );
}

function SectionLast({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <section style={{ paddingBottom: '4px' }}>
      <h3 className="text-[13px] font-semibold mb-3" style={{ color: 'var(--text-primary)' }}>{title}</h3>
      <div className="text-[12px] leading-relaxed" style={{ color: 'var(--text-secondary)' }}>
        {children}
      </div>
    </section>
  );
}

function DocP({ children }: { children: React.ReactNode }) {
  return <p className="mb-2">{children}</p>;
}

export default function UserManual({ isOpen, onClose }: UserManualProps) {
  return (
    <Modal isOpen={isOpen} onClose={onClose} title="用户手册" width={680}>
      {/* ==================== 1. 概述 ==================== */}
      <Section title="一、概述">
        <DocP>
          Viap 是一款 Windows 应用管理与存储重定向工具。它通过 <strong>NTFS 目录联结（Junction）</strong> 技术，
          将已安装的应用或大型数据文件夹从系统盘（通常是 C 盘）迁移到其他磁盘，同时在原始位置创建一个
          "重定向点"，使操作系统和应用本身都认为文件仍在原位。
        </DocP>
        <DocP>
          核心优势：<strong>应用无需重新安装，功能完全不受影响</strong>，用户无感知。
        </DocP>
      </Section>

      {/* ==================== 2. 迁移原理 ==================== */}
      <Section title="二、迁移：NTFS 目录联结原理">
        <DocP>
          <strong>目录联结（Junction）</strong> 是 Windows NTFS 文件系统原生支持的符号链接类型。
          它在文件系统层面创建一个 "指针"：当任何程序访问原始路径时，
          系统自动将请求重定向到目标位置。这对所有应用程序完全透明。
        </DocP>

        <DocP>
          <strong>迁移流程（5 步）：</strong>
        </DocP>
        <ol className="list-decimal pl-5 mb-3 space-y-1">
          <li>将原始文件夹中的所有文件完整复制到用户选择的目标位置</li>
          <li>验证复制完整性，确保无数据丢失</li>
          <li>将原始文件夹重命名为备份</li>
          <li>在原始位置创建 Junction，指向目标位置</li>
          <li>确认新路径可正常访问后，删除备份</li>
        </ol>
        <DocP>
          恢复流程反之：删除 Junction → 将文件从目标位置移回原位。
        </DocP>

        {/* 与 Windows 自带功能的区别 */}
        <div style={{ marginTop: '16px' }}>
          <DocP>
            <strong style={{ color: 'var(--text-primary)' }}>
              与 Windows "右键 → 属性 → 位置 → 移动" 的区别：
            </strong>
          </DocP>

          <div className="rounded p-3 mb-3 text-[11px] leading-relaxed"
            style={{ background: 'var(--color-warning-light)', border: '1px solid var(--color-warning)' }}>
            <strong style={{ color: 'var(--color-warning)' }}>Windows "移动文件夹" 的局限：</strong>
            <ul className="list-disc pl-4 mt-1 space-y-0.5" style={{ color: 'var(--text-secondary)' }}>
              <li>仅适用于系统预定义的特殊文件夹（桌面、文档、下载等），普通应用目录无法使用</li>
              <li>本质是修改注册表中的 Shell Folder 路径，而非文件系统级重定向</li>
              <li>某些应用可能忽略此设置，直接写入硬编码路径</li>
              <li>移动过程如果中断，可能导致文件夹位置不一致</li>
            </ul>
          </div>

          <div className="rounded p-3 mb-3 text-[11px] leading-relaxed"
            style={{ background: 'var(--color-primary-light)', border: '1px solid var(--color-primary)' }}>
            <strong style={{ color: 'var(--color-primary)' }}>Viap Junction 的优势：</strong>
            <ul className="list-disc pl-4 mt-1 space-y-0.5" style={{ color: 'var(--text-secondary)' }}>
              <li>适用于<strong>任意文件夹</strong>，包括已安装应用、游戏数据、聊天记录等</li>
              <li>文件系统级别的重定向，对所有程序透明，100% 兼容</li>
              <li>原始路径保持不变，应用无需任何配置修改</li>
              <li>可随时恢复，操作可逆</li>
            </ul>
          </div>

          <DocP>
            <strong>一句话总结：</strong>Windows 自带功能是"告诉系统文件夹换了个地方"，
            Viap 是"在文件系统底层做了一个透明的跳转"——后者更底层、更通用。
          </DocP>
        </div>
      </Section>

      {/* ==================== 3. 强力卸载 ==================== */}
      <Section title="三、强力卸载">
        <DocP>
          Viap 的卸载功能设计参考了 <strong>Geek Uninstaller</strong> 等专业卸载工具的标准流程，
          比 Windows 自带的"设置 → 应用 → 卸载"更加彻底。
        </DocP>

        <DocP>
          <strong>卸载流程（6 步）：</strong>
        </DocP>
        <ol className="list-decimal pl-5 mb-3 space-y-1">
          <li>
            <strong>读取注册表卸载命令：</strong>从系统注册表中获取应用的原生 UninstallString，
            确保使用应用官方提供的卸载程序。
          </li>
          <li>
            <strong>运行原始卸载向导：</strong>直接启动应用的卸载程序，让用户通过卸载向导正常交互
            （与 Geek Uninstaller 行为一致，不静默注入参数、不跳过用户确认）。
          </li>
          <li>
            <strong>等待卸载完成：</strong>监控卸载进程，等待其正常退出。
            如果卸载程序需要管理员权限，会自动通过 PowerShell 提权重试。
          </li>
          <li>
            <strong>确认卸载结果：</strong>检查注册表中该应用的条目是否已被移除。
          </li>
          <li>
            <strong>残留扫描：</strong>卸载完成后，自动扫描以下位置：
            <ul className="list-disc pl-5 mt-1 space-y-0.5">
              <li><code style={{ color: 'var(--color-primary)' }}>%APPDATA%</code> — 应用的用户配置和数据</li>
              <li><code style={{ color: 'var(--color-primary)' }}>%LOCALAPPDATA%</code> — 本地缓存和临时文件</li>
              <li><code style={{ color: 'var(--color-primary)' }}>%PROGRAMDATA%</code> — 全局应用数据</li>
              <li>注册表残留项</li>
            </ul>
          </li>
          <li>
            <strong>清理确认：</strong>将扫描到的残留列出并默认全选，用户确认后删除（支持回收站或彻底删除两种模式）。
          </li>
        </ol>

        <div className="rounded p-3 mb-3 text-[11px] leading-relaxed"
          style={{ background: 'var(--color-primary-light)', border: '1px solid var(--color-primary)' }}>
          <strong style={{ color: 'var(--color-primary)' }}>与 Windows 自带卸载的对比：</strong>
          <ul className="list-disc pl-4 mt-1 space-y-0.5" style={{ color: 'var(--text-secondary)' }}>
            <li>Windows 自带卸载仅运行卸载程序，<strong>不进行残留扫描</strong></li>
            <li>许多应用卸载后会在 AppData 留下数 GB 的配置/缓存文件，Windows 不会提示</li>
            <li>Viap 在卸载后自动扫描三大数据目录 + 注册表，彻底清除残留</li>
          </ul>
        </div>

        <DocP>
          <strong>安全机制：</strong>所有残留删除操作均有 4 层安全校验，
          确保不会误删系统文件或其他应用的数据。删除的文件可选择放入回收站，
          提供额外的安全保障。
        </DocP>
      </Section>

      {/* ==================== 4. 其他功能 ==================== */}
      <Section title="四、其他功能">
        <DocP>
          <strong>数据迁移（文件夹迁移）：</strong>管理常见的大型数据文件夹（微信/QQ 聊天记录、系统桌面/文档、
          下载目录、VS Code 扩展等），支持一键迁移到其他磁盘。同时支持添加自定义文件夹进行管理。
        </DocP>
        <DocP>
          <strong>迁移记录：</strong>记录所有迁移操作，支持查看详情、检查目标位置是否可用、
          一键恢复迁移。如果目标磁盘被移除或路径损坏，会显示异常状态。
        </DocP>
        <DocP>
          <strong>幽灵链接清理：</strong>当目标磁盘被移除或手动删除迁移后的文件夹后，
          Junction 指向的目标将不存在，成为"幽灵链接"。此功能扫描并清理这些失效的记录。
        </DocP>
        <DocP>
          <strong>应用管理（应用迁移）：</strong>扫描系统中已安装的应用，将其安装目录迁移到其他磁盘。
          迁移后应用仍可正常启动、更新和卸载。适用于 C 盘空间不足时，将大型应用（如 IDE、游戏）移出系统盘。
        </DocP>
      </Section>

      {/* ==================== 5. 数据安全 ==================== */}
      <Section title="五、数据安全说明">
        <DocP>
          迁移过程遵循 <strong>"先复制、后验证、再替换"</strong> 的安全流程：
        </DocP>
        <ul className="list-disc pl-5 mb-3 space-y-0.5">
          <li><strong>迁移前：</strong>检测是否有程序正在占用文件夹，防止迁移过程中文件被修改</li>
          <li><strong>复制阶段：</strong>完整复制所有文件到目标位置，保留目录结构</li>
          <li><strong>验证阶段：</strong>确认目标位置文件完整且可访问</li>
          <li><strong>替换阶段：</strong>将原始文件夹重命名为备份后才创建 Junction</li>
          <li><strong>回滚能力：</strong>如果在任何阶段出错，可从备份恢复</li>
        </ul>
        <DocP>
          <strong>建议：</strong>迁移重要数据前，建议先手动备份。虽然 Viap 设计了完整的安全机制，
          但任何涉及文件操作的软件都无法完全排除意外情况（如突然断电、磁盘故障）。
        </DocP>
      </Section>

      {/* ==================== 6. 使用协议 ==================== */}
      <SectionLast title="六、使用协议">
        <div className="rounded p-4 text-[11px] leading-relaxed"
          style={{ background: 'var(--bg-row-hover)', border: '1px solid var(--border-color-strong)' }}>
          <p className="mb-2 font-semibold" style={{ color: 'var(--text-primary)' }}>
            使用本软件即表示您已阅读并同意以下条款：
          </p>
          <ol className="list-decimal pl-5 space-y-1.5" style={{ color: 'var(--text-secondary)' }}>
            <li>
              本软件（Viap）是免费工具，按"现状"提供，不提供任何形式的明示或暗示担保。
            </li>
            <li>
              使用者应自行评估迁移操作的风险。本软件开发者对因使用本软件而导致的任何数据丢失、
              系统损坏、应用异常或其他直接或间接损失<strong>不承担任何责任</strong>。
            </li>
            <li>
              迁移操作涉及文件系统的底层修改。强烈建议在操作前关闭相关应用，
              并在迁移重要数据前进行独立备份。
            </li>
            <li style={{ color: 'red' }}>
              请勿将本软件用于迁移系统关键目录（如 Windows 目录、Edge浏览器、硬件驱动、Program Files 中的系统组件等），
              此类操作可能导致系统不稳定、甚至崩溃。
            </li>
            <li>
              本软件不会收集、上传或分享您的任何个人数据。所有数据均存储在本地。
            </li>
            <li>
              继续使用本软件即表示您已充分理解上述条款，并同意自行承担使用过程中的所有风险。
            </li>
          </ol>
        </div>
      </SectionLast>
    </Modal>
  );
}
