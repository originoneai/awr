import { describe, expect, it } from 'vitest';
import SNAPSHOTS from '../../../test-fixtures/work-state/snapshots-run09.json';
import { findForbiddenVisibleText } from '../../../lib/human-language-guard';
import { absoluteTimeText, deriveWorkState } from '../index';

const IN_PROGRESS = SNAPSHOTS['api-in-progress-with-live-report'];
const DELIVERED = SNAPSHOTS['api-delivered'];
const IN_REVIEW = SNAPSHOTS['api-in-review-owned-by-other'];
const zh = { language: 'zh-CN', readState: 'ready' } as const;

describe('deriveWorkState: next step', () => {
  it('never shows the service\'s agent-addressed operation name as the next step, and keeps it and its note verbatim in the technical layer', () => {
    for (const snapshot of [IN_PROGRESS, DELIVERED, IN_REVIEW]) {
      const state = deriveWorkState({ ...zh, snapshot });
      expect(state.next).toMatchObject({ kind: 'missing', text: '未报告下一步' });
      expect(state.next.technical.guidanceOp).toBe(snapshot.guidance.action.op);
      expect(state.next.technical.guidanceNote).toBe(snapshot.guidance.action.note);
      expect(state.next.text).not.toContain(String(snapshot.guidance.action.op));
      expect(findForbiddenVisibleText(state.next.text)).toEqual([]);
    }
  });

  it('shows the next step its reporter gave, the work\'s own first and the overview row\'s as the fallback', () => {
    expect(deriveWorkState({ ...zh, snapshot: { ...IN_PROGRESS, next_step: 'Review the contract' }, workNextStep: 'row text' }).next)
      .toMatchObject({ kind: 'reported', text: 'Review the contract' });
    expect(deriveWorkState({ ...zh, snapshot: IN_PROGRESS, workNextStep: 'row text' }).next)
      .toMatchObject({ kind: 'reported', text: 'row text' });
    expect(deriveWorkState({ ...zh, snapshot: { ...IN_PROGRESS, next_step: '' }, workNextStep: 'row text' }).next.text).toBe('row text');
  });

  it('says nothing was read while loading, and falls back to the row alone after a failed read', () => {
    const loading = deriveWorkState({ language: 'zh-CN', readState: 'loading', snapshot: { ...IN_PROGRESS, next_step: 'ignored' }, workNextStep: 'ignored' });
    expect(loading.next).toMatchObject({ kind: 'loading', text: '加载中…' });
    expect(loading.next.technical).toEqual({ nextStep: null, guidanceOp: null, guidanceNote: null });
    const failed = deriveWorkState({ language: 'zh-CN', readState: 'failed', snapshot: { ...IN_PROGRESS, next_step: 'must not be used' }, workNextStep: 'row text' });
    expect(failed.next).toMatchObject({ kind: 'reported', text: 'row text' });
    expect(deriveWorkState({ language: 'zh-CN', readState: 'failed', workNextStep: null }).next).toMatchObject({ kind: 'missing', text: '未报告下一步' });
  });
});

describe('deriveWorkState: last report', () => {
  it('shows the reported time and marks a report the service itself declared stale', () => {
    const stale = deriveWorkState({ ...zh, snapshot: IN_PROGRESS }).lastReport;
    const at = IN_PROGRESS.observation.reporting.client_reported_at_unix_ms;
    expect(stale).toMatchObject({ kind: 'measured', atUnixMs: at, stale: true });
    expect(stale.text).toBe(`${absoluteTimeText(at, { language: 'zh-CN' }).text}（可能已过期）`);
    const fresh = deriveWorkState({ ...zh, snapshot: IN_REVIEW }).lastReport;
    expect(fresh).toMatchObject({ kind: 'measured', stale: false });
    expect(fresh.text).toBe(absoluteTimeText(IN_REVIEW.observation.reporting.client_reported_at_unix_ms, { language: 'zh-CN' }).text);
    expect(deriveWorkState({ language: 'en-US', readState: 'ready', snapshot: IN_PROGRESS }).lastReport.text).toMatch(/ \(May be outdated\)$/);
  });

  it('says not read, with the freshness word, when no report time exists or the read failed, and never uses the snapshot of a failed read', () => {
    for (const reporting of [undefined, null, {}, { client_reported_at_unix_ms: null }, { client_reported_at_unix_ms: 'soon' }, { client_reported_at_unix_ms: Number.NaN }]) {
      const result = deriveWorkState({ ...zh, snapshot: { observation: { reporting } } }).lastReport;
      expect(result, JSON.stringify(reporting)).toMatchObject({ kind: 'not_measured', text: '尚未读取', atUnixMs: null });
    }
    expect(deriveWorkState({ language: 'zh-CN', readState: 'failed', snapshot: IN_REVIEW }).lastReport).toMatchObject({ kind: 'not_measured', text: '尚未读取', atUnixMs: null });
    expect(deriveWorkState({ language: 'zh-CN', readState: 'loading', snapshot: IN_REVIEW }).lastReport).toMatchObject({ kind: 'loading', text: '加载中…' });
  });

  it('words English the same way', () => {
    const state = deriveWorkState({ language: 'en-US', readState: 'ready', snapshot: DELIVERED });
    expect(state.next.text).toBe('No next step reported');
    expect(deriveWorkState({ language: 'en-US', readState: 'loading' }).next.text).toBe('Loading…');
    expect(deriveWorkState({ language: 'en-US', readState: 'ready', snapshot: { observation: {} } }).lastReport.text).toBe('Not read yet');
  });
});
