export interface TargetExistsConflict {
  isRetry: boolean;
  existingPath: string;
}

/**
 * 后端使用英文前缀作为迁移控制协议，前端必须先解析再展示，
 * 避免把 TARGET_EXISTS_RETRY 这类内部状态码暴露给普通用户。
 */
export function parseTargetExistsConflict(message: string): TargetExistsConflict | null {
  if (message.startsWith('TARGET_EXISTS_RETRY:')) {
    return {
      isRetry: true,
      existingPath: message.replace('TARGET_EXISTS_RETRY:', '').trim(),
    };
  }

  if (message.startsWith('TARGET_EXISTS:')) {
    return {
      isRetry: false,
      existingPath: message.replace('TARGET_EXISTS:', '').trim(),
    };
  }

  return null;
}

/**
 * 将后端迁移内部协议和兜底错误统一转成中文用户提示。
 * 控制协议仍保留给流程判断使用，但任何进入 UI 的文本都应走这里。
 */
export function formatMigrationFailureMessage(message: string): string {
  const safeMessage = message.trim();
  if (!safeMessage) return '迁移失败：后端未返回具体原因，请查看日志后重试。';

  const targetConflict = parseTargetExistsConflict(safeMessage);
  if (targetConflict) {
    return targetConflict.isRetry
      ? `目标位置已存在同名目录：\n${targetConflict.existingPath}\n\n该目录可能是上次迁移未完成留下的残留，也可能是手动创建的数据。请确认该目录可覆盖后再重试。`
      : `目标位置已存在同名目录：\n${targetConflict.existingPath}\n\n为避免覆盖已有数据，已停止迁移。请更换迁移目录，或确认该目录可覆盖后重试。`;
  }

  if (safeMessage.startsWith('JUNCTION_LOOP:')) {
    const targetPath = safeMessage.replace('JUNCTION_LOOP:', '').trim();
    return `检测到原路径仍是指向目标盘的目录链接，无法覆盖迁移。\n\n请先在「迁移记录」中恢复该项目，再重新迁移。\n\n目标位置：${targetPath}`;
  }

  if (safeMessage.startsWith('REQUIRES_WARNING_CONFIRM:')) {
    const detail = safeMessage.replace('REQUIRES_WARNING_CONFIRM:', '').trim();
    return `该目录属于高风险路径，需要通过风险确认后才能迁移。\n\n${detail}`;
  }

  if (safeMessage.startsWith('SYMLINK_FAILED_DATA_AT_TARGET:')) {
    const payload = safeMessage.replace('SYMLINK_FAILED_DATA_AT_TARGET:', '').trim();
    const [targetPath = '', ...detailLines] = payload.split(/\r?\n/);
    const details = detailLines.join('\n').trim();
    return details || `目录链接创建失败，数据已保留在目标位置：\n${targetPath}\n\n请确认目标数据完整后，再手动处理原路径或重试恢复。`;
  }

  return safeMessage;
}
