import { describe, expect, it } from 'vitest';
import REPORTS from '../../../test-fixtures/work-state/progress-reports-run09.json';
import { findForbiddenVisibleText } from '../../../lib/human-language-guard';
import { abbreviateIds, absoluteTimeText, describeProgressReport, moreLinesText, REPORT_LIST_LIMIT } from '../index';

const DELIVERED = REPORTS['api-delivered'];
const IN_REVIEW = REPORTS['api-in-review-owned-by-other'];
const zh = { language: 'zh-CN' } as const;
const en = { language: 'en-US' } as const;

function deepFreeze<T>(value: T): T {
  if (value && typeof value === 'object') {
    Object.values(value).forEach(deepFreeze);
    Object.freeze(value);
  }
  return value;
}

describe('describeProgressReport: the real checkpoints', () => {
  it('shows the delivered work\'s own sentences with long ids abbreviated, its test in words, and who said it and when', () => {
    const report = describeProgressReport(DELIVERED, zh);
    expect(report.kind).toBe('reported');
    expect(report.summary).toBe(abbreviateIds(DELIVERED.summary));
    expect(report.summary).toContain('from a5163424… to 5f7eb2fe… and finalized issue-filter-api');
    expect(report.completed).toEqual({ shown: DELIVERED.completed.map(abbreviateIds), more: 0 });
    expect(report.completed.shown[0]).toBe('delivery.integration.prepare 01M6B0NN… confirmed applied');
    const { technical, ...visible } = report;
    expect(JSON.stringify(visible)).not.toContain('5f7eb2fe9c563e4d355aa64a527e0aef754116c1');
    expect(JSON.stringify(technical)).toContain('5f7eb2fe9c563e4d355aa64a527e0aef754116c1');
    expect(report.blockers).toEqual({ shown: [], more: 0, reported: true });
    expect(report.tests).toEqual({ shown: [{ name: '本地内容与测试', outcome: '已通过', tone: 'positive' }], more: 0 });
    expect(report.artifacts).toEqual({ shown: [], more: 0 });
    expect(report.declaration).toEqual([
      `Project manager Agent 于 ${absoluteTimeText(DELIVERED.reported_at_unix_ms, { language: 'zh-CN' }).text} 汇报`,
      '内容由汇报者自述，系统未核验',
    ]);
    expect(report.contractNote).toBeNull();
  });

  it('shows the in-review work\'s tests with their outcome words and only the labels of what it produced', () => {
    const report = describeProgressReport(IN_REVIEW, zh);
    expect(report.tests.shown.map((test) => [test.name, test.outcome, test.tone])).toEqual([
      ['npm test', '已通过', 'positive'],
      ['本地内容与测试', '已通过', 'positive'],
    ]);
    expect(report.artifacts.shown).toEqual(['development branch', 'review round']);
    expect(JSON.stringify(report.artifacts)).not.toContain('refs/heads');
    expect(report.declaration[0]).toContain('Lin Agent');
  });

  it('keeps every field of the report in the technical items, verbatim, under its own name', () => {
    for (const checkpoint of [DELIVERED, IN_REVIEW]) {
      const { items } = describeProgressReport(checkpoint, zh).technical;
      expect(items.map((item) => item.label)).toEqual(Object.keys(checkpoint).map((key) => `progress.${key}`));
      const byLabel = Object.fromEntries(items.map((item) => [item.label, item.value]));
      expect(byLabel['progress.checkpoint_id']).toBe(checkpoint.checkpoint_id);
      expect(byLabel['progress.summary']).toBe(checkpoint.summary);
      expect(JSON.parse(byLabel['progress.completed'] as string)).toEqual(checkpoint.completed);
      expect(byLabel['progress.reported_contract_hash']).toBe(checkpoint.reported_contract_hash);
      expect(byLabel['progress.stale']).toBe('false');
      expect(byLabel['progress.reported_at_unix_ms']).toBe(String(checkpoint.reported_at_unix_ms));
      expect(JSON.parse(byLabel['progress.tests'] as string)).toEqual(checkpoint.tests);
      expect(byLabel['progress.client_observed_at_unix_ms']).toBeNull();
    }
  });

  it('does not change what it is given', () => {
    const frozen = deepFreeze(structuredClone(IN_REVIEW));
    expect(() => describeProgressReport(frozen, zh)).not.toThrow();
  });
});

describe('describeProgressReport: nothing to show', () => {
  it('says missing, with nothing else, for a value that is not a report or has no content to show', () => {
    for (const value of [undefined, null, 'text', 7, [], {}, { next_action: 'x', phase: 'testing' }, { completed: [], tests: [], artifacts: [] }, { completed: [' ', 3, null] }]) {
      const report = describeProgressReport(value, zh);
      expect(report, JSON.stringify(value)).toMatchObject({ kind: 'missing', summary: null, declaration: [], contractNote: null });
      expect(report.technical.items).toEqual([]);
    }
  });
});

describe('describeProgressReport: lists', () => {
  it('shows at most five entries of a list, says how many more there are, and counts only real sentences', () => {
    const lines = Array.from({ length: 7 }, (_, index) => `step ${index + 1}`);
    const report = describeProgressReport({ completed: [...lines.slice(0, 3), '', '  ', 4, null, ...lines.slice(3)] }, zh);
    expect(report.completed).toEqual({ shown: lines.slice(0, REPORT_LIST_LIMIT), more: 2 });
    expect(moreLinesText(2, 'zh-CN')).toBe('还有 2 条');
    expect(moreLinesText(2, 'en-US')).toBe('2 more');
  });

  it('knows whether the blocker list was given at all, so that an empty list can say none were reported', () => {
    expect(describeProgressReport({ summary: 's', blockers: [] }, zh).blockers).toEqual({ shown: [], more: 0, reported: true });
    expect(describeProgressReport({ summary: 's' }, zh).blockers.reported).toBe(false);
    expect(describeProgressReport({ summary: 's', blockers: 'none' }, zh).blockers.reported).toBe(false);
    expect(describeProgressReport({ summary: 's', blockers: ['waiting for the API', ''] }, zh).blockers).toEqual({ shown: ['waiting for the API'], more: 0, reported: true });
  });

  it('words each test outcome, an outcome it does not know as unrecognized, and a missing one as not provided', () => {
    const report = describeProgressReport({
      tests: [
        { name: 'unit', outcome: 'passed', reference: 'ref-1' },
        { name: 'e2e', outcome: 'failed' },
        { name: 'lint', outcome: 'skipped' },
        { name: 'smoke' },
        { outcome: 'passed' },
        'not an object',
        null,
      ],
    }, zh);
    expect(report.tests.shown).toEqual([
      { name: 'unit', outcome: '已通过', tone: 'positive' },
      { name: 'e2e', outcome: '失败', tone: 'negative' },
      { name: 'lint', outcome: '未识别状态', tone: 'neutral' },
      { name: 'smoke', outcome: '未提供', tone: 'neutral' },
      { name: '检查 5', outcome: '已通过', tone: 'positive' },
    ]);
    expect(describeProgressReport({ tests: [{ name: 'unit', outcome: 'passed' }] }, en).tests.shown[0].outcome).toBe('Passed');
  });

  it('shows a machine-named test as a numbered check by its place in the reporter\'s list, and keeps the name whole in the technical items', () => {
    const tests = [{ name: 'unit_tests', outcome: 'passed' }, 'not an object', { name: 'ci.build.lint', outcome: 'failed' }, { name: 'npm test', outcome: 'passed' }];
    const report = describeProgressReport({ tests }, zh);
    expect(report.tests.shown.map((test) => test.name)).toEqual(['检查 1', '检查 3', 'npm test']);
    expect(findForbiddenVisibleText(report.tests.shown.map((test) => test.name).join(' '))).toEqual([]);
    expect(describeProgressReport({ tests }, en).tests.shown.map((test) => test.name)).toEqual(['Check 1', 'Check 3', 'npm test']);
    const raw = report.technical.items.find((item) => item.label === 'progress.tests')?.value as string;
    expect(JSON.parse(raw)).toEqual(tests);
  });

  it('leaves the guard nothing to flag in any test name of the real checkpoints', () => {
    for (const checkpoint of [DELIVERED, IN_REVIEW]) {
      for (const options of [zh, en]) {
        const names = describeProgressReport(checkpoint, options).tests.shown.map((test) => test.name);
        expect(names.length).toBeGreaterThan(0);
        expect(findForbiddenVisibleText(names.join(' ')), options.language).toEqual([]);
      }
    }
  });

  it('shows only the labels of artifacts and skips entries that have none', () => {
    const report = describeProgressReport({ artifacts: [{ label: 'report', reference: 'sha-1' }, { reference: 'orphan' }, 'plain', { label: ' ' }] }, zh);
    expect(report.artifacts.shown).toEqual(['report']);
  });
});

describe('describeProgressReport: who said it, and whether it still holds', () => {
  const base = { summary: 'Done.' };

  it('names the reporter and the time, or says "the reporter" and no time when they are missing', () => {
    const at = 1793513588923;
    const time = absoluteTimeText(at, { language: 'zh-CN' }).text;
    expect(describeProgressReport({ ...base, origin: { actor_name: 'Lin Agent' }, reported_at_unix_ms: at }, zh).declaration).toEqual([`Lin Agent 于 ${time} 汇报`]);
    expect(describeProgressReport({ ...base, origin: { actor_name: ' ' }, reported_at_unix_ms: at }, zh).declaration).toEqual([`汇报者 于 ${time} 汇报`]);
    expect(describeProgressReport({ ...base, origin: { actor_name: 'Lin Agent' } }, zh).declaration).toEqual(['Lin Agent 汇报']);
    expect(describeProgressReport({ ...base, reported_at_unix_ms: 'soon', origin: null }, zh).declaration).toEqual(['汇报者 汇报']);
    const english = describeProgressReport({ ...base, origin: { actor_name: 'Lin Agent' }, reported_at_unix_ms: at }, en).declaration;
    expect(english).toEqual([`Reported by Lin Agent on ${absoluteTimeText(at, { language: 'en-US' }).text}`]);
  });

  it('says "not verified" only when the service says the reporter declared it, and "may be outdated" only for a stale report of open work', () => {
    expect(describeProgressReport({ ...base, provenance: 'caller_declared' }, zh).declaration.at(-1)).toBe('内容由汇报者自述，系统未核验');
    expect(describeProgressReport({ ...base, provenance: 'system_observed' }, zh).declaration).toHaveLength(1);
    expect(describeProgressReport({ ...base }, zh).declaration).toHaveLength(1);
    expect(describeProgressReport({ ...base, stale: true }, zh).declaration.at(-1)).toBe('可能已过期');
    expect(describeProgressReport({ ...base, stale: true }, { ...zh, finished: false }).declaration.at(-1)).toBe('可能已过期');
    expect(describeProgressReport({ ...base, stale: true }, { ...zh, finished: true }).declaration).toHaveLength(1);
    expect(describeProgressReport({ ...base, stale: false }, zh).declaration).toHaveLength(1);
    expect(describeProgressReport({ ...base, stale: 'yes' }, zh).declaration).toHaveLength(1);
  });

  it('warns only when the service says the task changed after the report was written', () => {
    expect(describeProgressReport({ ...base, contract_matches_current: false }, zh).contractNote).toBe('这份汇报写成之后，任务要求有变化，内容可能已不适用');
    expect(describeProgressReport({ ...base, contract_matches_current: false }, en).contractNote).toBe('The task requirements changed after this report was written; it may no longer apply');
    for (const value of [true, null, undefined, 'false', 0]) {
      expect(describeProgressReport({ ...base, contract_matches_current: value }, zh).contractNote, String(value)).toBeNull();
    }
  });

  it('uses no engineering words in anything it words itself, in either language', () => {
    const report = {
      summary: 'Plain sentence.',
      completed: ['One thing'],
      blockers: [],
      tests: [{ name: 'unit', outcome: 'passed' }, { name: 'e2e', outcome: 'skipped' }],
      artifacts: [{ label: 'branch' }],
      provenance: 'caller_declared',
      stale: true,
      contract_matches_current: false,
      reported_at_unix_ms: 1793513588923,
      origin: { actor_name: 'Lin Agent' },
    };
    for (const language of ['zh-CN', 'en-US']) {
      const result = describeProgressReport(report, { language });
      const words = [...result.declaration, result.contractNote ?? '', ...result.tests.shown.map((test) => test.outcome), moreLinesText(3, language)];
      expect(findForbiddenVisibleText(words.join('\n')), language).toEqual([]);
    }
  });
});
