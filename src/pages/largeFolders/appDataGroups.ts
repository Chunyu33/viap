import type { LargeFolder } from '../../types';
import type { AppDataGroup, AppDataGroupId } from './types';

const GROUP_DEFINITIONS: Array<Pick<AppDataGroup, 'id' | 'title'>> = [
  { id: 'social', title: '社交软件' },
  { id: 'browser', title: '浏览器与缓存' },
  { id: 'developer', title: '开发环境' },
  { id: 'creative', title: '创意设计' },
  { id: 'ai', title: 'AI 工具' },
  { id: 'other', title: '其他应用数据' },
];

/** 只维护模板 ID 与分类的映射，避免显示名称变化导致分类失效。 */
const GROUP_ID_BY_FOLDER_ID: Record<string, AppDataGroupId> = {
  wechat: 'social',
  wxwork: 'social',
  qq: 'social',
  dingtalk: 'social',
  feishu: 'social',
  chrome_cache: 'browser',
  edge_cache: 'browser',
  vscode_extensions: 'developer',
  vscode_user_data: 'developer',
  cursor_appdata: 'developer',
  cursor_extensions: 'developer',
  npm_global: 'developer',
  npm_cache: 'developer',
  yarn_cache: 'developer',
  gradle_cache: 'developer',
  maven_repository: 'developer',
  cargo_home: 'developer',
  rustup_home: 'developer',
  pip_cache: 'developer',
  uv_cache: 'developer',
  nuget_packages: 'developer',
  docker_data: 'developer',
  dotnet_data: 'developer',
  adobe_appdata: 'creative',
  adobe_localdata: 'creative',
  jianying_appdata: 'creative',
  jianying_localdata: 'creative',
  claude_code: 'ai',
  codex_data: 'ai',
  devin_data: 'ai',
  ollama_data: 'ai',
  comfyui_data: 'ai',
  gemini_data: 'ai',
};

/** 按 ID 一次性建索引，避免每个分类重复遍历完整列表。 */
export function groupAppDataFolders(folders: LargeFolder[]): AppDataGroup[] {
  const foldersByGroup = new Map<AppDataGroupId, LargeFolder[]>();
  for (const folder of folders) {
    const groupId = GROUP_ID_BY_FOLDER_ID[folder.id] ?? 'other';
    const groupFolders = foldersByGroup.get(groupId);
    if (groupFolders) {
      groupFolders.push(folder);
    } else {
      foldersByGroup.set(groupId, [folder]);
    }
  }

  return GROUP_DEFINITIONS
    .map((definition) => ({ ...definition, folders: foldersByGroup.get(definition.id) ?? [] }))
    .filter((group) => group.folders.length > 0);
}
