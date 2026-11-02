import { readdirSync, readFileSync, statSync } from 'node:fs';
import { join } from 'node:path';
import { describe, expect, it } from 'vitest';
import { FIELD_LABELS, STATUS_LABELS, fieldLabel, formatMetric, formatObservationValue, notProvided, statusLabel } from './i18n/craft';

function filesUnder(path: string): string[] {
  return readdirSync(path).flatMap((name) => {
    const file = join(path, name);
    return statSync(file).isDirectory() ? filesUnder(file) : [file];
  });
}

describe('craft copy vocabulary', () => {
  it('covers every raw Descriptions field found by a source scan', () => {
    const files = [...filesUnder('src/pages'), ...filesUnder('src/components')].filter((file) => file.endsWith('.tsx'));
    const missing = new Set<string>();
    for (const file of files) {
      const source = readFileSync(file, 'utf8');
      if (!source.includes('<Descriptions')) continue;
      const keys = [
        ...source.matchAll(/label:\s*'([a-z][a-z0-9_.]+)'/g),
        ...source.matchAll(/^\s*\['([a-z][a-z0-9_.]+)',\s*/gm),
      ].map((match) => match[1]);
      for (const key of keys) if (!FIELD_LABELS[key]) missing.add(`${file}:${key}`);
    }
    expect([...missing]).toEqual([]);
  });

  it('does not leave raw snake_case labels in Descriptions', () => {
    const files = [...filesUnder('src/pages'), ...filesUnder('src/components')].filter((file) => file.endsWith('.tsx'));
    const offenders = files.filter((file) => {
      const source = readFileSync(file, 'utf8');
      return source.includes('<Descriptions') && /label:\s*'[a-z][a-z0-9_.]+'/.test(source);
    });
    expect(offenders).toEqual([]);
  });

  it('uses one bilingual status vocabulary without mixed-language examples', () => {
    const examples = {
      approved: ['已批准', 'Approved'], rejected: ['已拒绝', 'Rejected'], invalidated: ['已失效', 'Invalidated'],
      open: ['进行中', 'Open'], confirmed: ['已确认', 'Confirmed'], dispatched: ['已分派', 'Dispatched'],
      committed: ['已提交', 'Committed'], succeeded: ['成功', 'Succeeded'], unknown: ['未知', 'Unknown'],
    };
    for (const [status, [zh, en]] of Object.entries(examples)) {
      expect(statusLabel(status, 'zh-CN', 'state')).toBe(zh);
      expect(statusLabel(status, 'en-US', 'state')).toBe(en);
      expect(STATUS_LABELS[status]).toEqual([zh, en]);
      expect(zh).not.toMatch(/[A-Za-z]/);
      expect(en).not.toMatch(/[\u4e00-\u9fff]/);
    }
  });

  it('formats metrics, booleans and missing facts consistently', () => {
    expect(formatMetric('1234567', 'en-US', 'items_count')).toBe('1,234,567');
    expect(formatMetric('1', 'zh-CN', 'selection_version')).toBe('1');
    expect(formatObservationValue(true, 'zh-CN')).toBe('是');
    expect(formatObservationValue(false, 'en-US')).toBe('No');
    expect(formatObservationValue('approved', 'zh-CN', 'state')).toBe('已批准');
    expect(notProvided('zh-CN')).toBe('未提供');
    expect(fieldLabel('dispatch_state', 'en-US')).toBe('Dispatch state');
  });
});
