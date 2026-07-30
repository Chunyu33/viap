import type { LargeFolder } from '../../types';

/** 应用数据二级分类，使用稳定的前端标识而不是显示名称。 */
export type AppDataGroupId = 'social' | 'browser' | 'developer' | 'creative' | 'ai' | 'other';

/** 应用数据分类及其条目，供手风琴组件消费。 */
export interface AppDataGroup {
  id: AppDataGroupId;
  title: string;
  folders: LargeFolder[];
}
