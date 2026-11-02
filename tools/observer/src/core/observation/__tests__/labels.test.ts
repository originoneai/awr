import { describe, expect, it } from 'vitest';
import DERIVED_RUN09 from '../../../test-fixtures/activity/derived-audit-development-project-scope-all-run09.json';
import { findForbiddenVisibleText } from '../../../lib/human-language-guard';
import REPORTS from '../../../test-fixtures/work-state/progress-reports-run09.json';
import { actorAliases, checkLabel, workLabel } from '../index';

const ITEMS = DERIVED_RUN09.data.items as Array<{ actor_id: string; work_id: string | null }>;
const ACTORS = ITEMS.map((item) => item.actor_id);

describe('actorAliases', () => {
  it('numbers the real run-09 actors the same way whatever order the events arrive in', () => {
    const forward = actorAliases(ACTORS, { language: 'zh-CN' });
    const backward = actorAliases([...ACTORS].reverse(), { language: 'zh-CN' });
    expect(forward.entries.length).toBeGreaterThan(1);
    expect(forward.entries).toEqual(backward.entries);
    expect(forward.entries.map((entry) => entry.label)).toEqual(forward.entries.map((_, index) => `操作者 ${index + 1}`));
    for (const entry of forward.entries) expect(findForbiddenVisibleText(entry.label)).toEqual([]);
  });

  it('shows the viewer as "You" first and numbers the others from 1', () => {
    const self = ACTORS[0];
    const result = actorAliases(ACTORS, { language: 'zh-CN', selfActorId: self });
    expect(result.entries[0]).toEqual({ actorId: self, label: '你', isSelf: true });
    expect(result.entries[1].label).toBe('操作者 1');
    expect(result.labelOf(self)).toBe('你');
    expect(actorAliases(ACTORS, { language: 'en-US', selfActorId: self }).labelOf(self)).toBe('You');
  });

  it('does not invent a "You" entry for a viewer who has no event in the list', () => {
    const result = actorAliases(['b', 'a'], { language: 'en-US', selfActorId: 'nobody' });
    expect(result.entries.map((entry) => [entry.actorId, entry.label])).toEqual([['a', 'Operator 1'], ['b', 'Operator 2']]);
  });

  it('lists each operator once however often it appears', () => {
    const result = actorAliases(['a', 'a', 'b', 'a', 'b'], { language: 'zh-CN' });
    expect(result.entries.map((entry) => entry.actorId)).toEqual(['a', 'b']);
  });

  it('says plainly when an operator is missing or unknown instead of naming one', () => {
    const result = actorAliases(['a', '', null, undefined], { language: 'zh-CN' });
    expect(result.entries).toHaveLength(1);
    expect(result.labelOf(null)).toBe('未提供操作者');
    expect(result.labelOf('')).toBe('未提供操作者');
    expect(result.labelOf('never-listed')).toBe('未提供操作者');
  });
});

describe('workLabel', () => {
  it('shows a readable work name as it is and keeps the raw value for technical details', () => {
    expect(workLabel('issue-filter-page', { language: 'zh-CN' })).toEqual({ kind: 'named', text: 'issue-filter-page', technical: { raw: 'issue-filter-page' } });
    const realNamed = ITEMS.filter((item) => item.work_id).map((item) => workLabel(item.work_id, { language: 'zh-CN' }));
    expect(realNamed.length).toBeGreaterThan(0);
    expect(realNamed.every((label) => label.kind === 'named')).toBe(true);
  });

  it('never shows an opaque identifier as a name', () => {
    for (const opaque of ['01M6A0PZ0CB6PZ6X8JQ4TSFQWN', 'issue_filter_page', 'work.item.one', 'a'.repeat(64), '8f04de65e537a0eeda7069f71e858b6fbedcb695b54c2846b1d7942e5953784f']) {
      const result = workLabel(opaque, { language: 'zh-CN' });
      expect(result, opaque).toEqual({ kind: 'unnamed', text: '某项工作', technical: { raw: opaque } });
    }
    expect(workLabel('01M6A0PZ0CB6PZ6X8JQ4TSFQWN', { language: 'en-US' }).text).toBe('A work item');
  });

  it('says there is no related work for a missing id, never the freshness word', () => {
    for (const missing of [null, undefined, '']) {
      const result = workLabel(missing, { language: 'zh-CN' });
      expect(result).toEqual({ kind: 'none', text: '无关联工作', technical: { raw: null } });
      expect(result.text).not.toBe('尚未读取');
    }
    expect(workLabel(null, { language: 'en-US' }).text).toBe('No related work');
  });
});

describe('checkLabel', () => {
  const zh = { language: 'zh-CN' } as const;
  const en = { language: 'en-US' } as const;

  it('words the machine name of the real local check in plain words, in both languages, and keeps the name', () => {
    expect(checkLabel('local_git.manifest', { ...zh, position: 2 })).toEqual({ kind: 'known', text: '本地内容与测试', technical: { raw: 'local_git.manifest' } });
    expect(checkLabel('local_git.manifest', { ...en, position: 2 }).text).toBe('Local content and tests');
  });

  it('shows a name that is plain words exactly as the reporter wrote it', () => {
    for (const name of ['npm test', 'Unit tests', '单元测试', 'build and lint']) {
      expect(checkLabel(name, { ...zh, position: 1 }), name).toEqual({ kind: 'named', text: name, technical: { raw: name } });
    }
    expect(checkLabel('  npm test  ', { ...zh, position: 1 }).text).toBe('npm test');
  });

  it('numbers every other name the human-language guard would flag by its place in the list and keeps the name for the technical details', () => {
    for (const name of ['unit_tests', 'ci.build.lint', 'api.contract_check', 'Snapshot test']) {
      const result = checkLabel(name, { ...zh, position: 3 });
      expect(result, name).toEqual({ kind: 'numbered', text: '检查 3', technical: { raw: name } });
      expect(findForbiddenVisibleText(result.text)).toEqual([]);
    }
    expect(checkLabel('unit_tests', { ...en, position: 3 }).text).toBe('Check 3');
  });

  it('numbers a check that has no usable name and has nothing to keep', () => {
    for (const missing of [undefined, null, '', '   ', 7, true, {}, ['x']]) {
      expect(checkLabel(missing, { ...zh, position: 4 }), JSON.stringify(missing)).toEqual({ kind: 'numbered', text: '检查 4', technical: { raw: null } });
    }
    expect(checkLabel(undefined, { ...en, position: 1 }).text).toBe('Check 1');
  });

  it('never gives the guard anything to flag across every check of the real checkpoints, in both languages', () => {
    const names = Object.values(REPORTS).flatMap((report) => ((report as { tests?: Array<{ name?: unknown }> }).tests ?? []).map((test) => test.name));
    expect(names).toContain('local_git.manifest');
    expect(names).toContain('npm test');
    names.forEach((name, index) => {
      for (const language of ['zh-CN', 'en-US']) {
        expect(findForbiddenVisibleText(checkLabel(name, { language, position: index + 1 }).text), `${String(name)} ${language}`).toEqual([]);
      }
    });
  });
});
