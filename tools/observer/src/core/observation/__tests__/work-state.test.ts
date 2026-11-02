import { describe, expect, it } from 'vitest';
import SNAPSHOTS from '../../../test-fixtures/work-state/snapshots-run09.json';
import { findForbiddenVisibleText } from '../../../lib/human-language-guard';
import { absoluteTimeText, deriveWorkState } from '../index';

const IN_PROGRESS = SNAPSHOTS['api-in-progress-with-live-report'];
const DELIVERED = SNAPSHOTS['api-delivered'];
const IN_REVIEW = SNAPSHOTS['api-in-review-owned-by-other'];
const zh = { language: 'zh-CN', readState: 'ready' } as const;

describe('deriveWorkState: next step', () => {
  it('shows the next action the reporter wrote in the checkpoint, verbatim, and keeps the service\'s operation and note out of the text but verbatim in the technical layer', () => {
    for (const snapshot of [DELIVERED, IN_REVIEW]) {
      const state = deriveWorkState({ ...zh, snapshot });
      expect(state.next).toMatchObject({ kind: 'reported', text: snapshot.observation.progress.next_action });
      expect(state.next.technical).toEqual({
        nextAction: snapshot.observation.progress.next_action,
        nextStep: null,
        guidanceOp: snapshot.guidance.action.op,
        guidanceNote: snapshot.guidance.action.note,
      });
      expect(state.next.text).not.toContain(String(snapshot.guidance.action.op));
      expect(state.next.text).not.toBe(snapshot.guidance.action.note);
    }
  });

  it('says plainly that no next step was reported when the checkpoint gave none, never the operation name or the service\'s own hint', () => {
    const state = deriveWorkState({ ...zh, snapshot: IN_PROGRESS });
    expect(state.next).toMatchObject({ kind: 'missing', text: '未报告下一步' });
    expect(state.next.technical.guidanceOp).toBe(IN_PROGRESS.guidance.action.op);
    expect(state.next.text).not.toContain(String(IN_PROGRESS.guidance.action.op));
    expect(findForbiddenVisibleText(state.next.text)).toEqual([]);
    const hinted = deriveWorkState({ ...zh, snapshot: { ...IN_PROGRESS, next_step: 'Call work.prepare to restore the context.' } });
    expect(hinted.next).toMatchObject({ kind: 'missing', text: '未报告下一步' });
    expect(hinted.next.technical.nextStep).toBe('Call work.prepare to restore the context.');
  });

  it('prefers the checkpoint\'s next action over the overview row\'s, and falls back to the row when the checkpoint has no sentence', () => {
    const withProgress = (next_action: unknown) => ({ ...IN_PROGRESS, observation: { ...IN_PROGRESS.observation, progress: { next_action } } });
    expect(deriveWorkState({ ...zh, snapshot: withProgress('Review the contract'), workNextStep: 'row text' }).next)
      .toMatchObject({ kind: 'reported', text: 'Review the contract' });
    expect(deriveWorkState({ ...zh, snapshot: withProgress('  Review the contract \n'), workNextStep: 'row text' }).next.text).toBe('Review the contract');
    expect(deriveWorkState({ ...zh, snapshot: IN_PROGRESS, workNextStep: 'row text' }).next)
      .toMatchObject({ kind: 'reported', text: 'row text' });
    for (const blank of ['', '   ', null, 7, { text: 'x' }]) {
      const state = deriveWorkState({ ...zh, snapshot: withProgress(blank), workNextStep: 'row text' });
      expect(state.next, JSON.stringify(blank)).toMatchObject({ kind: 'reported', text: 'row text' });
    }
    expect(deriveWorkState({ ...zh, snapshot: withProgress({ text: 'x' }) }).next).toMatchObject({ kind: 'missing', text: '未报告下一步' });
    expect(deriveWorkState({ ...zh, snapshot: withProgress({ text: 'x' }) }).next.technical.nextAction).toBe('{"text":"x"}');
  });

  it('says nothing was read while loading, and falls back to the row alone after a failed read', () => {
    const loading = deriveWorkState({ language: 'zh-CN', readState: 'loading', snapshot: IN_REVIEW, workNextStep: 'ignored' });
    expect(loading.next).toMatchObject({ kind: 'loading', text: '加载中…' });
    expect(loading.next.technical).toEqual({ nextAction: null, nextStep: null, guidanceOp: null, guidanceNote: null });
    const failed = deriveWorkState({ language: 'zh-CN', readState: 'failed', snapshot: IN_REVIEW, workNextStep: 'row text' });
    expect(failed.next).toMatchObject({ kind: 'reported', text: 'row text' });
    expect(failed.next.technical).toEqual({ nextAction: null, nextStep: null, guidanceOp: null, guidanceNote: null });
    expect(deriveWorkState({ language: 'zh-CN', readState: 'failed', snapshot: IN_REVIEW, workNextStep: null }).next).toMatchObject({ kind: 'missing', text: '未报告下一步' });
  });
});

describe('deriveWorkState: last report', () => {
  it('shows the reported time and marks a report the service itself declared stale', () => {
    const stale = deriveWorkState({ ...zh, snapshot: IN_PROGRESS }).lastReport;
    const at = IN_PROGRESS.observation.reporting.client_reported_at_unix_ms;
    const time = absoluteTimeText(at, { language: 'zh-CN' }).text;
    expect(stale).toMatchObject({ kind: 'measured', atUnixMs: at, stale: true, timeText: time, staleText: '可能已过期' });
    expect(stale.text).toBe(`${time}（可能已过期）`);
    const fresh = deriveWorkState({ ...zh, snapshot: IN_REVIEW }).lastReport;
    const freshTime = absoluteTimeText(IN_REVIEW.observation.reporting.client_reported_at_unix_ms, { language: 'zh-CN' }).text;
    expect(fresh).toMatchObject({ kind: 'measured', stale: false, timeText: freshTime, staleText: null, text: freshTime });
    const english = deriveWorkState({ language: 'en-US', readState: 'ready', snapshot: IN_PROGRESS }).lastReport;
    expect(english.staleText).toBe('May be outdated');
    expect(english.text).toBe(`${english.timeText} (May be outdated)`);
  });

  it('says not read, with the freshness word, when no report time exists or the read failed, and never uses the snapshot of a failed read', () => {
    for (const reporting of [undefined, null, {}, { client_reported_at_unix_ms: null }, { client_reported_at_unix_ms: 'soon' }, { client_reported_at_unix_ms: Number.NaN }]) {
      const result = deriveWorkState({ ...zh, snapshot: { observation: { reporting } } }).lastReport;
      expect(result, JSON.stringify(reporting)).toMatchObject({ kind: 'not_measured', text: '尚未读取', timeText: null, staleText: null, atUnixMs: null });
    }
    expect(deriveWorkState({ language: 'zh-CN', readState: 'failed', snapshot: IN_REVIEW }).lastReport).toMatchObject({ kind: 'not_measured', text: '尚未读取', atUnixMs: null });
    expect(deriveWorkState({ language: 'zh-CN', readState: 'loading', snapshot: IN_REVIEW }).lastReport).toMatchObject({ kind: 'loading', text: '加载中…' });
  });

  it('words English the same way', () => {
    const state = deriveWorkState({ language: 'en-US', readState: 'ready', snapshot: IN_PROGRESS });
    expect(state.next.text).toBe('No next step reported');
    expect(deriveWorkState({ language: 'en-US', readState: 'ready', snapshot: DELIVERED }).next.text).toBe(DELIVERED.observation.progress.next_action);
    expect(deriveWorkState({ language: 'en-US', readState: 'loading' }).next.text).toBe('Loading…');
    expect(deriveWorkState({ language: 'en-US', readState: 'ready', snapshot: { observation: {} } }).lastReport.text).toBe('Not read yet');
  });
});
