import { describe, expect, it } from 'vitest';
import { deriveTrouble } from './core/observation';

/** Synthetic snapshots of the recorded shape (the run's own values stay out of the repo). */
const rejected = (targetChanged: unknown) => ({
  observation: {
    collaboration: { integration: { state: 'rejected', current_binding: true, target_changed: targetChanged, id: '01M6SYNTHETICINTEGRATION000000A' } },
  },
});

describe('deriveTrouble', () => {
  it('reports a rejected integration on the current binding, with the target-moved sentence only when it moved', () => {
    const moved = deriveTrouble({ snapshot: rejected(true), readState: 'ready', options: { language: 'zh-CN' } });
    expect(moved).toHaveLength(1);
    expect(moved[0].kind).toBe('integration-rejected');
    expect(moved[0].text).toBe('并入主线被退回，主线已变化');
    const plain = deriveTrouble({ snapshot: rejected(false), readState: 'ready', options: { language: 'zh-CN' } });
    expect(plain[0].text).toBe('并入主线被退回');
    const en = deriveTrouble({ snapshot: rejected(true), readState: 'ready', options: { language: 'en-US' } });
    expect(en[0].text).toBe('The merge into the mainline was rejected, and the mainline has moved');
  });

  it('says nothing for a confirmed integration, a stale binding, or no integration at all', () => {
    const confirmed = { observation: { collaboration: { integration: { state: 'confirmed', current_binding: true } } } };
    const stale = { observation: { collaboration: { integration: { state: 'rejected', current_binding: false } } } };
    const missing = { observation: {} };
    for (const snapshot of [confirmed, stale, missing]) {
      expect(deriveTrouble({ snapshot, readState: 'ready', options: { language: 'zh-CN' } })).toEqual([]);
    }
  });

  it('reports a failed last execution, and only the word failed — no other terminal word is guessed', () => {
    const failed = { observation: { execution: { state: 'failed', execution_id: '01M6SYNTHETICEXECUTION00000A' } } };
    const facts = deriveTrouble({ snapshot: failed, readState: 'ready', options: { language: 'zh-CN' } });
    expect(facts).toHaveLength(1);
    expect(facts[0].text).toBe('上一次尝试没有成功');
    for (const state of ['succeeded', 'aborted', 'cancelled', 'timeout']) {
      const other = { observation: { execution: { state } } };
      expect(deriveTrouble({ snapshot: other, readState: 'ready', options: { language: 'zh-CN' } })).toEqual([]);
    }
  });

  it('carries the raw values for the technical details and stays silent on reads that did not happen', () => {
    const facts = deriveTrouble({ snapshot: rejected(true), readState: 'ready', options: { language: 'zh-CN' } });
    expect(facts[0].technical['collaboration.integration.id']).toBe('01M6SYNTHETICINTEGRATION000000A');
    expect(deriveTrouble({ snapshot: rejected(true), readState: 'loading', options: { language: 'zh-CN' } })).toEqual([]);
    expect(deriveTrouble({ snapshot: rejected(true), readState: 'failed', options: { language: 'zh-CN' } })).toEqual([]);
    expect(deriveTrouble({ snapshot: null, readState: 'ready', options: { language: 'zh-CN' } })).toEqual([]);
  });
});
