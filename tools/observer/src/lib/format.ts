import { absoluteTimeText } from '../core/observation';

/** UI compatibility wrapper; core owns absolute-time derivation. */
export function formatUnixMs(unixMs: number, locale?: string): string {
  return absoluteTimeText(unixMs, { language: locale ?? 'zh-CN' }).text;
}
