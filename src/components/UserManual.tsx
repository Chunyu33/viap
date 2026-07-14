// 用户手册组件
// 内容按逻辑顺序编排：概述 → 迁移原理 → 强力卸载 → 其他功能 → 数据安全 → 使用协议

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
          Viap 是一款 Windows 应用管理与存储重定向工具。它通过 <strong>NTFS 目录链接</strong> 技术
          （同盘优先 Junction、跨盘自动软链接），将已安装的应用或大型数据文件夹从系统盘
          （通常是 C 盘）迁移到其他磁盘，同时在原始位置创建一个"重定向点"，
          使操作系统和应用本身都认为文件仍在原位。
        </DocP>
        <DocP>
          核心优势：多数普通应用无需重新安装即可继续使用；对含系统服务、驱动、商店包或强路径校验的软件，
          Viap 会尽量拦截或提示风险。
        </DocP>
      </Section>

      {/* ==================== 2. 迁移原理 ==================== */}
      <Section title="二、迁移：NTFS 目录链接原理">
        <DocP>
          Viap 使用 Windows NTFS 原生目录链接技术进行重定向：
        </DocP>
        <ul className="list-disc pl-5 mb-3 space-y-1">
          <li>
            <strong>应用/文件夹迁移：</strong>优先使用 <strong>Junction（目录联结）</strong>。
            Junction 是 NTFS 原生的轻量级目录重定向，通常无需管理员权限，适合把 C 盘应用迁移到其他 NTFS 磁盘。
          </li>
          <li>
            <strong>特殊场景：</strong>如果遇到系统权限、目标文件系统或路径类型不支持的情况，
            Viap 会停止操作并给出错误提示，不会在未完成验证时直接替换原路径。
          </li>
        </ul>
        <DocP>
          两种链接在文件系统层面效果一致：当任何程序访问原始路径时，系统自动将请求重定向到目标位置，
          对大多数普通应用透明。
        </DocP>

        <DocP>
          <strong>迁移流程：</strong>
        </DocP>
        <ol className="list-decimal pl-5 mb-3 space-y-1">
          <li>将原始文件夹中的所有文件完整复制到用户选择的目标位置</li>
          <li>验证复制完整性，确保无数据丢失</li>
          <li>删除原始文件夹</li>
          <li>在原始位置创建目录链接，指向目标位置</li>
          <li>写入迁移记录，完成</li>
        </ol>
        <DocP>
          恢复流程反之：删除链接 → 将文件从目标位置移回原位。
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
            <strong style={{ color: 'var(--color-primary)' }}>Viap 目录链接的优势：</strong>
            <ul className="list-disc pl-4 mt-1 space-y-0.5" style={{ color: 'var(--text-secondary)' }}>
              <li>适用于<strong>任意文件夹</strong>，包括已安装应用、游戏数据、聊天记录等</li>
              <li>文件系统级别的重定向，对大多数普通程序透明</li>
              <li>原始路径保持不变，应用无需任何配置修改</li>
              <li>可随时恢复，操作可逆</li>
            </ul>
          </div>

          <DocP>
            <strong>一句话总结：</strong>Windows 自带功能是"告诉系统文件夹换了个地方"，
            Viap 是"在文件系统底层做了一个透明的跳转"。它适合普通应用和数据目录，
            但不建议用于系统管控目录、驱动、商店应用和带强自修复机制的软件。
          </DocP>
        </div>
      </Section>

      {/* ==================== 3. 已知局限性 ==================== */}
      <Section title="三、已知局限性（非万能方案）">
        <DocP>
          Viap 的目录链接方案并非万能，以下类型的软件在迁移后<strong>可能出现异常甚至无法使用</strong>。
          这不是 Viap 的缺陷，而是由软件自身的安装机制决定的。
        </DocP>

        <div className="rounded p-3 mb-3 text-[11px] leading-relaxed"
          style={{ background: 'var(--color-warning-light)', border: '1px solid var(--color-warning)' }}>
          <strong style={{ color: 'var(--color-warning)' }}>不要直接迁移整个 AppData：</strong>
          <DocP>
            不建议把 <code style={{ color: 'var(--color-warning)' }}>%APPDATA%</code>、
            <code style={{ color: 'var(--color-warning)' }}>%LOCALAPPDATA%</code>，或其中的
            <code style={{ color: 'var(--color-warning)' }}>Local</code>、
            <code style={{ color: 'var(--color-warning)' }}>Roaming</code> 等公共目录整体迁移。
            AppData 由多个软件和 Windows 组件共享，目录中同时存在配置、缓存、登录凭据、锁文件、运行时状态和临时文件，
            整体建立链接会把不相关的软件一起绑定到新位置，增加启动失败和数据损坏的范围。
          </DocP>
          <DocP>
            建议按照“软件颗粒度”迁移明确的数据目录，例如单独迁移微信、浏览器缓存或开发工具缓存，并先退出相关软件。
            带许可证校验、系统服务、内核驱动、反作弊组件或强路径校验的软件，可能把路径写入服务、注册表、授权文件或驱动配置，
            也可能在更新/自修复时删除目录链接；这类软件即使目录复制成功，也不适合使用软链接或 Junction 重定向。
          </DocP>
        </div>

        <div className="rounded p-3 mb-3 text-[11px] leading-relaxed"
          style={{ background: 'var(--color-danger-light)', border: '1px solid var(--color-danger)' }}>
          <strong style={{ color: 'var(--color-danger)' }}>绝对不可迁移（已自动拦截）：</strong>
          <ul className="list-disc pl-4 mt-1 space-y-1" style={{ color: 'var(--text-secondary)' }}>
            <li>
              <strong>Microsoft Office（ClickToRun）：</strong>
              Office 使用 ClickToRun 虚拟化文件系统，安装路径写进 COM 注册和激活记录。
              其自我修复服务会把 Junction 识别为损坏安装并自动覆盖，迁移无效且可能触发重新安装。
            </li>
            <li>
              <strong>浏览器（Edge / Chrome 等）：</strong>
              浏览器安装目录含系统级自动修复服务，更新或修复时会将 Junction 替换为实际目录，
              所有扩展插件也将因路径签名变化而损坏。
            </li>
            <li>
              <strong>GPU 显卡驱动（NVIDIA / AMD / Intel）：</strong>
              驱动 DLL 路径硬编码在系统服务注册表中，迁移后驱动无法加载，轻则降级到基本显示，重则蓝屏。
            </li>
            <li>
              <strong>.NET Runtime：</strong>
              运行时路径被大量应用的 runtimeconfig.json 和系统环境变量硬编码引用，
              迁移后所有依赖 .NET 的应用将无法启动。
            </li>
            <li>
              <strong>Windows 系统目录：</strong>
              Windows、System32、WinSxS 等系统核心目录含大量硬链接和内核级依赖，迁移会导致系统崩溃无法开机。
            </li>
            <li>
              <strong>微软商店应用（Microsoft Store / UWP / MSIX）：</strong>
              通常位于 <code style={{ color: 'var(--color-danger)' }}>C:\Program Files\WindowsApps</code>，
              由 AppX 部署服务、包签名、ACL 权限和商店更新机制共同管理。强行通过目录链接迁移会导致应用无法启动、
              商店更新失败或权限损坏，因此 Viap 不将其作为可迁移应用处理。部分商店应用可在 Windows 设置中使用
              “移动”按钮，或通过存储设置调整新应用默认保存位置，应优先使用这些系统能力。
            </li>
          </ul>
        </div>

        <div className="rounded p-3 mb-3 text-[11px] leading-relaxed"
          style={{ background: 'var(--color-warning-light)', border: '1px solid var(--color-warning)' }}>
          <strong style={{ color: 'var(--color-warning)' }}>高风险类型（迁移前需评估并完全停止相关服务）：</strong>
          <ul className="list-disc pl-4 mt-1 space-y-1" style={{ color: 'var(--text-secondary)' }}>
            <li>
              <strong>安全软件（杀毒/防火墙）：</strong>
              含内核级驱动组件，路径写进系统服务注册表，迁移后防护功能可能失效，需重新安装。
            </li>
            <li>
              <strong>数据库（MySQL / PostgreSQL / MongoDB / Redis 等）：</strong>
              数据目录含事务日志和锁文件，服务运行中迁移会损坏数据。需在完全停止服务后操作，
              迁移后可能还需修改配置文件中的数据目录路径。
            </li>
            <li>
              <strong>虚拟化软件（VMware / VirtualBox / Hyper-V）：</strong>
              虚拟磁盘和配置文件含绝对路径引用，迁移后虚拟机无法直接启动，需手动重新注册。
            </li>
            <li>
              <strong>开发工具（Visual Studio / JetBrains / VSCode）：</strong>
              含被系统内核持续映射的 DLL 和后台语言服务进程，复制阶段容易失败。
              迁移前需完全退出所有相关进程（包括系统托盘的 background service）。
            </li>
            <li>
              <strong>Steam 等游戏平台库：</strong>
              游戏库迁移后平台无法自动识别新路径，需在平台设置中手动添加新路径并重新扫描。
            </li>
          </ul>
        </div>

        <DocP>
          <strong>总结：</strong>Viap 最适合迁移"普通应用"——即那些不依赖系统级注册、
          不含内核驱动、不持续后台运行的应用。如遇上述高风险类型，Viap 会主动拦截或弹出风险提示。
          即使通过了检测，也建议在迁移前<strong>关闭目标应用并备份重要数据</strong>。
        </DocP>
      </Section>

      {/* ==================== 4. 强力卸载 ==================== */}
      <Section title="四、强力卸载">
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

      {/* ==================== 5. 其他功能 ==================== */}
      <Section title="五、其他功能">
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
          链接指向的目标将不存在，成为"幽灵链接"。此功能扫描并清理这些失效的记录。
        </DocP>
        <DocP>
          <strong>应用管理（应用迁移）：</strong>扫描系统中已安装的应用，将其安装目录迁移到其他磁盘。
          对普通桌面应用，迁移后通常仍可正常启动、更新和卸载。适用于 C 盘空间不足时，将大型应用（如 IDE、游戏）移出系统盘。
          微软商店应用由 Windows 包管理系统维护，不建议通过目录链接迁移；部分应用会在 Windows 设置中提供“移动”按钮，
          也可通过存储设置调整新应用默认保存位置，应优先使用 Windows 提供的迁移方式。
        </DocP>
      </Section>

      {/* ==================== 5. 数据安全 ==================== */}
      <Section title="六、数据安全说明">
        <DocP>
          迁移过程遵循 <strong>"先复制、后验证、再替换"</strong> 的安全流程：
        </DocP>
        <ul className="list-disc pl-5 mb-3 space-y-0.5">
          <li><strong>迁移前：</strong>检测是否有程序正在占用文件夹，防止迁移过程中文件被修改</li>
          <li><strong>复制阶段：</strong>完整复制所有文件到目标位置，保留目录结构</li>
          <li><strong>验证阶段：</strong>确认目标位置文件完整且可访问</li>
          <li><strong>替换阶段：</strong>删除原始文件夹后创建目录链接</li>
          <li><strong>失败保护：</strong>如果复制或验证阶段出错，会保留原路径，不会创建半成品链接</li>
        </ul>
        <DocP>
          <strong>建议：</strong>迁移重要数据前，建议先手动备份。虽然 Viap 设计了完整的安全机制，
          但任何涉及文件操作的软件都无法完全排除意外情况（如突然断电、磁盘故障）。
        </DocP>
      </Section>

      {/* ==================== 6. 使用协议 ==================== */}
      <SectionLast title="七、使用协议">
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
            <li className='text-red-500'>
              请勿删除本软件的数据存储目录，否则会丢失所有配置和迁移历史记录。
            </li>
            <li className='text-red-500'>
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
