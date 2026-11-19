import { readFileSync } from 'node:fs';
import path from 'node:path';
import { describe, expect, it } from 'vitest';
import { OVERVIEW_RULES, TASK_STATUSES, WAIT_KINDS } from '../index';

const documentPath = path.resolve(process.cwd(), '../../docs/dev/reference/overview-fact-semantics.md');
const documentText = readFileSync(documentPath, 'utf8');

function documentedRules() {
  const rows = new Map<string, string[]>();
  for (const line of documentText.split('\n')) {
    if (!line.startsWith('| `')) continue;
    const cells = line.split('|').map((cell) => cell.trim());
    const term = cells[1].match(/^`([^`]+)`$/)?.[1];
    if (!term) continue;
    const rules = [...cells[4].matchAll(/`([^`]+)`/g)].map((match) => match[1]);
    rows.set(term, rules);
  }
  return rows;
}

describe('documented fact semantics', () => {
  it('checks every documented rule row against the implementation contract, including shared service words', () => {
    const rows = documentedRules();
    expect(Object.keys(OVERVIEW_RULES).sort()).toEqual([...rows.keys()].sort());
    for (const [term, rules] of Object.entries(OVERVIEW_RULES)) {
      expect(rows.get(term), term).toEqual(rules);
    }
    for (const sharedWord of ['核验过', '标了完成', '等待', '压着']) {
      expect(documentText).toContain(sharedWord);
    }
  });

  it('keeps all nine states and all four wait kinds in the reviewed table', () => {
    const rows = documentedRules();
    for (const status of TASK_STATUSES) expect(rows.has(status), status).toBe(true);
    for (const kind of WAIT_KINDS) {
      const term = kind === 'external' ? 'externalWait' : `${kind}Wait`;
      expect(rows.has(term), term).toBe(true);
    }
  });

  it('keeps the two approval kinds documented as pending inputs beside waits and handoffs', () => {
    const rows = documentedRules();
    expect(rows.get('approvals')).toEqual(['approval-kinds-explicit', 'approval-recipient-explicit',
      'approval-project-scoped', 'decisions-exclude-approvals']);
    expect(documentText).toContain('never the viewer and never somebody else');
    expect(documentText).toContain('The top view\'s decision metric counts decisions only');
  });

  it('makes unknown, coverage and count/detail invariants explicit', () => {
    expect(documentText).toContain('count === items.length');
    expect(documentText).toContain('count: null');
    expect(documentText).toContain('there is no default');
    expect(documentText).toContain('unknown graph edges do not enter');
  });
});
