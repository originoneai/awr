import { describe, expect, it } from 'vitest';
import SNAPSHOTS from '../../../test-fixtures/work-state/snapshots-run09.json';
import { findForbiddenVisibleText } from '../../../lib/human-language-guard';
import { abbreviateIds, absoluteTimeText, deriveWorkState } from '../index';

const IN_PROGRESS = SNAPSHOTS['api-in-progress-with-live-report'];
const DELIVERED = SNAPSHOTS['api-delivered'];
const IN_REVIEW = SNAPSHOTS['api-in-review-owned-by-other'];
const zh = { language: 'zh-CN', readState: 'ready' } as const;

describe('deriveWorkState: next step', () => {
  it('shows the next action the reporter wrote in the checkpoint with long identifiers abbreviated, the sentence as written beside it, and keeps the service\'s operation and note out of the text but verbatim in the technical layer', () => {
    for (const snapshot of [DELIVERED, IN_REVIEW]) {
      const state = deriveWorkState({ ...zh, snapshot });
      expect(state.next).toMatchObject({ kind: 'reported', text: abbreviateIds(snapshot.observation.progress.next_action), fullText: snapshot.observation.progress.next_action });
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

  it('cuts the long ids of the reporter\'s sentence in the text, in the checkpoint\'s and in the row\'s alike, and keeps the sentence as written', () => {
    const review = deriveWorkState({ ...zh, snapshot: IN_REVIEW }).next;
    expect(review.text).toBe('Wait for independent review of round 01M6AVQ8…. Do not self-approve or merge to main.');
    expect(review.fullText).toBe('Wait for independent review of round 01M6AVQ870N7MBRJ5MPCN79T55. Do not self-approve or merge to main.');
    expect(review.technical.nextAction).toBe(review.fullText);
    const row = deriveWorkState({ ...zh, snapshot: { observation: {} }, workNextStep: 'Merge 5f7eb2fe9c563e4d355aa64a527e0aef754116c1 first' }).next;
    expect(row).toMatchObject({ kind: 'reported', text: 'Merge 5f7eb2fe… first', fullText: 'Merge 5f7eb2fe9c563e4d355aa64a527e0aef754116c1 first' });
    for (const state of [deriveWorkState({ ...zh, snapshot: IN_PROGRESS }).next, deriveWorkState({ language: 'zh-CN', readState: 'loading' }).next]) {
      expect(state.fullText).toBeNull();
    }
  });

  it('says the operation names the shared vocabulary knows inside the reporter\'s sentence in words, in both languages, and keeps the sentence as written', () => {
    const sentence = 'Run delivery.finalize for round 01M6AVQ870N7MBRJ5MPCN79T55';
    const snapshot = { observation: { progress: { next_action: sentence } } };
    expect(deriveWorkState({ ...zh, snapshot }).next).toMatchObject({ kind: 'reported', text: 'Run 完成交付 for round 01M6AVQ8…', fullText: sentence });
    expect(deriveWorkState({ language: 'en-US', readState: 'ready', snapshot }).next.text).toBe('Run Finalize delivery for round 01M6AVQ8…');
    expect(deriveWorkState({ ...zh, snapshot: { observation: {} }, workNextStep: 'Wait for delivery.integration.confirm' }).next.text).toBe('Wait for 确认并入主线');
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

  it('never puts a stale note on a finished work, by the runtime or by the reporter\'s phase, but keeps the service\'s flag verbatim', () => {
    const at = DELIVERED.observation.reporting.client_reported_at_unix_ms;
    const time = absoluteTimeText(at, { language: 'zh-CN' }).text;
    expect(DELIVERED.observation.reporting.client_stale).toBe(true);
    const finishedByRuntime = deriveWorkState({ ...zh, snapshot: DELIVERED }).lastReport;
    expect(finishedByRuntime).toMatchObject({ kind: 'measured', stale: true, finished: true, staleText: null, text: time, timeText: time });
    expect(finishedByRuntime.technical).toMatchObject({ atUnixMs: at, runtimeState: 'completed', phase: 'delivered' });
    // the reporter's own phase is enough when the runtime state is not final (or absent)
    const byPhase = { ...DELIVERED, runtime: { state: 'claimed' }, observation: { ...DELIVERED.observation, runtime: { state: 'claimed' } } };
    expect(deriveWorkState({ ...zh, snapshot: byPhase }).lastReport).toMatchObject({ finished: true, staleText: null, text: time });
    // the runtime state is enough when there is no checkpoint at all
    const byRuntime = { ...DELIVERED, observation: { ...DELIVERED.observation, progress: null } };
    expect(deriveWorkState({ ...zh, snapshot: byRuntime }).lastReport).toMatchObject({ finished: true, staleText: null, text: time });
    // a report on open work stays marked; the observation block's runtime state is read when the top level has none
    const open = { ...IN_PROGRESS, runtime: undefined, observation: { ...IN_PROGRESS.observation, runtime: { state: 'claimed' } } };
    expect(deriveWorkState({ ...zh, snapshot: open }).lastReport).toMatchObject({ finished: false, staleText: '可能已过期' });
    const closedByObservation = { ...IN_PROGRESS, runtime: undefined, observation: { ...IN_PROGRESS.observation, runtime: { state: 'completed' } } };
    expect(deriveWorkState({ ...zh, snapshot: closedByObservation }).lastReport).toMatchObject({ finished: true, staleText: null });
  });

  it('does not mark a report stale when the service said nothing about it, or said something that is not a yes or no', () => {
    const at = IN_REVIEW.observation.reporting.client_reported_at_unix_ms;
    const time = absoluteTimeText(at, { language: 'zh-CN' }).text;
    for (const reporting of [{ client_reported_at_unix_ms: at }, { client_reported_at_unix_ms: at, client_stale: null }, { client_reported_at_unix_ms: at, client_stale: 'yes' }]) {
      const report = deriveWorkState({ ...zh, snapshot: { observation: { reporting } } }).lastReport;
      expect(report, JSON.stringify(reporting)).toMatchObject({ kind: 'measured', stale: null, staleText: null, text: time });
    }
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

describe('deriveWorkState: progress word', () => {
  it('says the reporter\'s phase in plain words with the tone it carries, and the runtime state when the reporter gave no phase', () => {
    expect(deriveWorkState({ ...zh, snapshot: IN_REVIEW }).progress)
      .toMatchObject({ kind: 'phase', text: '待评审', tone: 'warning', technical: { phase: 'ready_for_review', runtimeState: 'claimed', rowStatus: null } });
    expect(deriveWorkState({ ...zh, snapshot: DELIVERED }).progress).toMatchObject({ kind: 'phase', text: '已交付', tone: 'positive' });
    expect(deriveWorkState({ ...zh, snapshot: IN_PROGRESS }).progress).toMatchObject({ kind: 'runtime', text: '已认领', tone: 'warning', technical: { phase: null, runtimeState: 'claimed' } });
    expect(deriveWorkState({ language: 'en-US', readState: 'ready', snapshot: IN_REVIEW }).progress.text).toBe('Ready for review');
  });

  it('has a word and a tone for each of the eleven phases the service defines, in both languages', () => {
    const phases: Record<string, [string, string, string]> = {
      starting: ['刚开始', 'Just started', 'neutral'],
      implementing: ['开发中', 'In development', 'neutral'],
      testing: ['测试中', 'Testing', 'neutral'],
      waiting_user: ['等待确认', 'Waiting for a decision', 'warning'],
      blocked: ['受阻', 'Blocked', 'negative'],
      ready_for_review: ['待评审', 'Ready for review', 'warning'],
      waiting_dependency: ['等待前置工作', 'Waiting on other work', 'warning'],
      reviewing: ['评审中', 'In review', 'warning'],
      reworking: ['返工中', 'Being reworked', 'warning'],
      integrating: ['合入中', 'Being integrated', 'neutral'],
      delivered: ['已交付', 'Delivered', 'positive'],
    };
    expect(Object.keys(phases)).toHaveLength(11);
    for (const [phase, [zhText, enText, tone]] of Object.entries(phases)) {
      // 'delivered' reads as delivered only with the system's completion behind it (see the next test)
      const snapshot = { runtime: { state: 'completed', selected_completion_id: 'completion-1' }, observation: { progress: { phase } } };
      expect(deriveWorkState({ ...zh, snapshot }).progress, phase).toMatchObject({ kind: 'phase', text: zhText, tone });
      expect(deriveWorkState({ language: 'en-US', readState: 'ready', snapshot }).progress.text, phase).toBe(enText);
      expect(findForbiddenVisibleText(zhText), phase).toEqual([]);
    }
  });

  it('takes the runtime state before a phase it does not know, and calls that phase unrecognized only when nothing else is known', () => {
    const odd = { ...IN_REVIEW, observation: { ...IN_REVIEW.observation, progress: { phase: 'brand_new_phase', next_action: 'x' } } };
    expect(deriveWorkState({ ...zh, snapshot: odd }).progress).toMatchObject({ kind: 'runtime', text: '已认领', technical: { phase: 'brand_new_phase', runtimeState: 'claimed' } });
    expect(deriveWorkState({ ...zh, snapshot: { observation: { progress: { phase: 'brand_new_phase' } } } }).progress)
      .toMatchObject({ kind: 'phase', text: '未识别状态', tone: 'neutral', technical: { phase: 'brand_new_phase' } });
  });

  it('falls back to the overview row\'s status when the read failed or says nothing, then to unknown, and never reads a failed snapshot', () => {
    expect(deriveWorkState({ language: 'zh-CN', readState: 'failed', snapshot: IN_REVIEW, workStatus: 'open' }).progress)
      .toMatchObject({ kind: 'row', text: '进行中', technical: { phase: null, runtimeState: null, rowStatus: 'open' } });
    expect(deriveWorkState({ ...zh, snapshot: { observation: {} }, workStatus: 'completed' }).progress).toMatchObject({ kind: 'row', text: '已完成', tone: 'positive' });
    expect(deriveWorkState({ ...zh, snapshot: { observation: {} }, workStatus: 'free text from somewhere' }).progress).toMatchObject({ kind: 'row', text: '未识别状态' });
    expect(deriveWorkState({ language: 'zh-CN', readState: 'failed', snapshot: IN_REVIEW }).progress).toMatchObject({ kind: 'unknown', text: '未知', tone: 'neutral' });
    expect(deriveWorkState({ language: 'zh-CN', readState: 'failed', snapshot: DELIVERED }).progress.technical).toEqual({ phase: null, runtimeState: null, rowStatus: null, completionId: null });
    expect(deriveWorkState({ ...zh, snapshot: IN_REVIEW, workStatus: 'open' }).progress.kind).toBe('phase');
    expect(deriveWorkState({ ...zh, snapshot: IN_PROGRESS, workStatus: 'open' }).progress.kind).toBe('runtime');
  });

  it('says loading while the read is pending, and nothing else', () => {
    expect(deriveWorkState({ language: 'zh-CN', readState: 'loading', snapshot: IN_REVIEW, workStatus: 'open' }).progress)
      .toEqual({ kind: 'loading', text: '加载中…', tone: 'neutral', asOfUnixMs: null, technical: { phase: null, runtimeState: null, rowStatus: null, completionId: null } });
  });

  it('keeps the time at which the service measured the facts behind the word, and none when nothing was read or the time is not a time', () => {
    const AS_OF = 1793519600000;
    const measured = (snapshot: typeof DELIVERED | typeof IN_PROGRESS | typeof IN_REVIEW) => ({ ...snapshot, observation: { ...snapshot.observation, observed_at_unix_ms: AS_OF } });
    for (const snapshot of [IN_PROGRESS, DELIVERED, IN_REVIEW]) {
      expect(deriveWorkState({ ...zh, snapshot: measured(snapshot) }).progress.asOfUnixMs).toBe(AS_OF);
    }
    expect(deriveWorkState({ language: 'zh-CN', readState: 'failed', snapshot: measured(DELIVERED), workStatus: 'open' }).progress.asOfUnixMs).toBeNull();
    expect(deriveWorkState({ language: 'zh-CN', readState: 'loading', snapshot: measured(DELIVERED) }).progress.asOfUnixMs).toBeNull();
    expect(deriveWorkState({ ...zh, snapshot: DELIVERED }).progress.asOfUnixMs).toBeNull();
    for (const bad of [null, undefined, 'yesterday', Number.NaN, Number.POSITIVE_INFINITY]) {
      const snapshot = { ...DELIVERED, observation: { ...DELIVERED.observation, observed_at_unix_ms: bad } };
      expect(deriveWorkState({ ...zh, snapshot }).progress.asOfUnixMs, String(bad)).toBeNull();
    }
    // an unknown word keeps its as-of too
    expect(deriveWorkState({ ...zh, snapshot: { observation: { observed_at_unix_ms: 1793513588923 } } }).progress)
      .toMatchObject({ kind: 'unknown', asOfUnixMs: 1793513588923 });
  });

  it('never words a work as active, completed or accepted from a claim, an execution or a review state', () => {
    const sample = {
      ...IN_PROGRESS,
      observation: {
        ...IN_PROGRESS.observation,
        claim: { state: 'active', lease_live: false },
        execution: { state: 'succeeded', effects_settled: true },
        collaboration: { review: { state: 'open' }, candidate: { current: true } },
      },
    };
    for (const language of ['zh-CN', 'en-US']) {
      const state = deriveWorkState({ language, readState: 'ready', snapshot: sample });
      const words = [state.progress.text, state.next.text, state.lastReport.text].join('\n');
      for (const claim of ['活跃', '已完成', '已验收', '已通过', 'active', 'completed', 'accepted', 'approved']) {
        expect(words.toLowerCase(), `${language}: ${claim}`).not.toContain(claim.toLowerCase());
      }
      expect(state.progress.kind).toBe('runtime');
    }
    // the completion is stated only for the delivered snapshot that holds the exact selected completion, with its as-of
    const stamped = { ...DELIVERED, observation: { ...DELIVERED.observation, observed_at_unix_ms: 1793519600000 } };
    const delivered = deriveWorkState({ ...zh, snapshot: stamped }).progress;
    expect(delivered).toMatchObject({ text: '已交付', asOfUnixMs: 1793519600000, technical: { completionId: DELIVERED.runtime.selected_completion_id } });
  });

  it('reads a reporter\'s "delivered" as delivered only when the system holds the completion, otherwise as reported', () => {
    const completion = DELIVERED.runtime.selected_completion_id;
    expect(completion).toBeTruthy();
    expect(deriveWorkState({ ...zh, snapshot: DELIVERED }).progress)
      .toMatchObject({ kind: 'phase', text: '已交付', tone: 'positive', technical: { phase: 'delivered', runtimeState: 'completed', completionId: completion } });
    const withRuntime = (state: string, selected: unknown) => ({
      ...DELIVERED,
      runtime: { state, selected_completion_id: selected },
      observation: { ...DELIVERED.observation, runtime: { state, selected_completion_id: selected } },
    });
    // the reporter says delivered, the system has no completion and the work is still claimed
    expect(deriveWorkState({ ...zh, snapshot: withRuntime('claimed', null) }).progress)
      .toMatchObject({ kind: 'phase', text: '汇报称已交付', tone: 'warning', technical: { completionId: null } });
    // the runtime state says completed but no completion is selected, or a blank one
    for (const selected of [null, undefined, '', '   ', 7]) {
      expect(deriveWorkState({ ...zh, snapshot: withRuntime('completed', selected) }).progress, String(selected))
        .toMatchObject({ text: '汇报称已交付', tone: 'warning' });
    }
    // a completion without the completed state is not a confirmation either
    expect(deriveWorkState({ ...zh, snapshot: withRuntime('claimed', completion) }).progress.text).toBe('汇报称已交付');
    // the observation block's runtime is read when the top level has none
    const observationOnly = { ...DELIVERED, runtime: undefined };
    expect(deriveWorkState({ ...zh, snapshot: observationOnly }).progress.text).toBe('已交付');
    // no reporter phase: the system's own completed state, with its completion, is the word
    const noPhase = { ...DELIVERED, observation: { ...DELIVERED.observation, progress: null } };
    expect(deriveWorkState({ ...zh, snapshot: noPhase }).progress).toMatchObject({ kind: 'runtime', text: '已完成', tone: 'positive' });
    expect(deriveWorkState({ language: 'en-US', readState: 'ready', snapshot: withRuntime('claimed', null) }).progress.text).toBe('Reported as delivered');
    expect(deriveWorkState({ language: 'en-US', readState: 'ready', snapshot: DELIVERED }).progress.text).toBe('Delivered');
  });
});
