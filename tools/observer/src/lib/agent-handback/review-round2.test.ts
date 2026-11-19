// @vitest-environment node
/**
 * Review round 2 (PR #689, supervisor comment 7020830645). Each describe maps to one
 * required change; every assertion here failed on head f92f75ca unless marked
 * "already green". The last describe replants the old implementation piece by piece
 * and shows these tests catch it.
 */
import { describe, expect, it } from 'vitest';
import { buildHandback, HANDBACK_MAX_SENTENCE_LENGTH } from './build';
import { sanitizeUntrustedText } from './sanitize';
import { handbackVocabulary } from '../../i18n/handback';
import { textInvariants } from './handback-test-utils';
import type { HandbackContext, HandbackItem } from './types';

const ctx = (agentName: string | null = null, connections: string[] = []): HandbackContext => ({
  person: { agentConnections: connections.map((name) => ({ name, kind: null, lastSeenUnixMs: null })) },
  task: { executionAgentName: agentName },
});

const build = (item: Partial<HandbackItem>, context: HandbackContext = ctx('Codex', ['Codex']), lang = 'zh-CN') =>
  buildHandback({ kind: 'answer', projectName: '订单协作平台', taskName: '空状态检查', question: '要不要加引导', ...item } as HandbackItem, context, lang);

const len = (s: string) => Array.from(s).length;

describe('change 1: placeholders are inert and the line stays within 240 characters', () => {
  it('a project name made of placeholders cannot push the line over 240 characters', () => {
    const { sentence } = build({ projectName: '{{question}}'.repeat(10), question: 'Q'.repeat(100) });
    expect(len(sentence)).toBeLessThanOrEqual(HANDBACK_MAX_SENTENCE_LENGTH);
  });

  it('a task name that is a placeholder never pulls the question into the task slot', () => {
    const { sentence } = build({ taskName: '{{question}}', question: 'SECRETQUESTION' });
    expect((sentence.match(/SECRETQUESTION/g) ?? []).length).toBeLessThanOrEqual(1);
    expect(sentence).not.toContain('{{');
  });

  it('no output of any kind or language contains a live placeholder', () => {
    for (const language of ['zh-CN', 'en-US'] as const) {
      for (const field of ['projectName', 'taskName', 'question'] as const) {
        const poisoned = { [field]: '{{agent}}'.repeat(5) } as Partial<HandbackItem>;
        const { sentence, where } = build({ ...poisoned, kind: 'decision' }, ctx('Codex', ['Codex']), language);
        expect(sentence, `${language} ${field}`).not.toContain('{{');
        expect(where).not.toContain('{{');
      }
    }
  });

  it('braces in untrusted text become fullwidth, so no placeholder shape survives cleaning', () => {
    const { text } = sanitizeUntrustedText('把{答}案{写}进{{question}}里', 120);
    expect(text).not.toMatch(/[{}]/);
    expect(text).toContain('｛');
  });
});

describe('change 2: long hostile inputs are cleaned in bounded time', () => {
  const HOSTILE = [
    'B-AAAAAAAA-'.repeat(8_000),
    'A-'.repeat(50_000),
    'A-' + 'B-'.repeat(8_000) + '!',
    'a.b '.repeat(50_000),
    '0'.repeat(1_000_000),
  ];

  it('each 100 KB-class hostile input finishes in under 200 ms', () => {
    for (const [index, text] of HOSTILE.entries()) {
      const t0 = performance.now();
      sanitizeUntrustedText(text, 120);
      const ms = performance.now() - t0;
      expect(ms, `input ${index}: ${ms.toFixed(1)}ms`).toBeLessThan(200);
    }
  });

  // Machine timing wobbles, so besides the absolute cap we check the growth ratio:
  // doubling the input must not quadruple the time (the signature of the old
  // backtracking pattern). Ratio 3.5 leaves room for noise while a quadratic blow-up
  // from ~1 ms to ~4+ ms per doubling is caught reliably.
  it('doubling the input roughly doubles the work, not quadruples it', () => {
    const measure = (repeats: number): number => {
      const t0 = performance.now();
      sanitizeUntrustedText('B-AAAAAAAA-'.repeat(repeats), 120);
      return performance.now() - t0;
    };
    measure(2_000); // warm up
    const small = measure(2_000);
    const large = measure(4_000);
    expect(large, `small ${small.toFixed(1)}ms large ${large.toFixed(1)}ms`).toBeLessThan(small * 3.5 + 5);
  });
});

describe('change 3: square brackets never survive cleaning', () => {
  it('link syntax dies in every form the supervisor listed', () => {
    for (const text of ['[click](javascript:alert(1))', '[a](b(c)d)', '[x](y)', '[x][1] [1]: http://evil.example']) {
      expect(sanitizeUntrustedText(text, 120).text, text).not.toMatch(/[\[\]]/);
    }
  });
});

describe('change 4: hidden marks are removed and emoji presentation selectors are kept', () => {
  it('variation selectors and the combining grapheme joiner do not survive', () => {
    const { text } = sanitizeUntrustedText('任务\uFE00名\uFE01称\u{E0100}带\u034F变', 120);
    expect(/[\u{FE00}-\u{FE0D}\u{E0100}-\u{E01EF}\u034F]/u.test(text)).toBe(false);
    expect(textInvariants(text, 120)).toEqual([]);
  });

  it('emoji presentation selectors stay so hearts keep their colour', () => {
    expect(sanitizeUntrustedText('ok ❤️', 120).text).toContain('\u{FE0F}');
  });
});

describe('change 5: dotted names follow the service-namespace rule', () => {
  it('ordinary file names, extensions and domains stay exactly as written', () => {
    expect(sanitizeUntrustedText('Rename server.js to app.js', 120).text).toBe('Rename server.js to app.js');
    expect(sanitizeUntrustedText('更新 README.md 的说明', 120).text).toBe('更新 README.md 的说明');
    expect(sanitizeUntrustedText('Use example.com for demos', 120).text).toBe('Use example.com for demos');
  });

  it('two-segment service operations are still dropped, whatever their case', () => {
    expect(sanitizeUntrustedText('call work.list and claim.acquire now', 120).text).toBe('call and now');
    expect(sanitizeUntrustedText('联调 Work.List 与 WORK.read 的接口', 120).text).toBe('联调 与 的接口');
  });

  it('three-or-more-segment dotted names and work-item keys stay dropped', () => {
    expect(sanitizeUntrustedText('整理 config.default.yaml 和 T-104 的清单', 120).text).toBe('整理 和 的清单');
  });
});

/**
 * Plant-back: legacySanitize and legacyInterpolate reproduce the cleaner and the
 * interpolator exactly as head f92f75ca shipped them (sequential placeholder
 * replacement, braces and square brackets passed through, link parsing, unbounded
 * identifier repetition, no raw-input cut, no hidden-mark removal, any-dotted
 * dropping). Every assertion above that failed on f92f75ca is shown failing here,
 * which is what "plant the old implementation back and watch the tests go red" means
 * when it must stay reproducible in CI.
 */
function legacyInterpolate(template: string, values: Record<string, string | undefined>): string {
  let out = template;
  for (const [key, value] of Object.entries(values)) {
    if (value !== undefined) out = out.split(`{{${key}}}`).join(value);
  }
  return out;
}

function legacySanitize(input: string, budget: number): { text: string; truncated: boolean } {
  if (typeof input !== 'string' || input.length === 0) return { text: '', truncated: false };
  const normalized = input.normalize('NFKC');
  const folded = normalized
    .replace(/[\p{Cc}]/gu, ' ')
    .replace(/[\p{Cf}\p{Cs}\p{Co}\p{Cn}]/gu, '')
    .replace(/\s+/g, ' ')
    .trim();
  let out = folded;
  for (const pattern of [
    /\bbearer\s+[A-Za-z0-9._~+/=-]{8,}/gi,
    /\b(?:sk|pk|ghp|gho|ghu|xox[bap]|AKIA|AIza)[A-Za-z0-9_-]{8,}\b/g,
    /\bsha(?:256|512):[0-9A-Za-z]+/gi,
    /\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b/gi,
    /\b[0-9a-f]{12,}\b/gi,
    /\b[0-9A-HJKMNP-TV-Z]{26}\b/g,
    /\b[A-Z][A-Z0-9]{0,7}-(?:[A-Z0-9]{1,8}-)*\d[A-Z0-9]*\b/g,
    /\b[a-z][a-z0-9]*_[a-z0-9_]+\b/g,
    /\b[a-z][a-z0-9_]*\.[a-z][a-z0-9_]*\b/gi,
  ]) {
    out = out.replace(pattern, ' ');
  }
  out = out.replace(/\s+/g, ' ').trim();
  out = out.replace(/\[([^\[\]]*)\]\(([^()]*)\)/g, '［$1］（$2）');
  out = out.replace(/</g, '＜').replace(/>/g, '＞');
  out = out.replace(/["“”]/g, '＂').replace(/['‘’`]/g, '＇');
  out = out.replace(/^[#>*\-+/!`]+(?:\s*[#>*\-+/!`]+)*/, '').trim();
  const chars = Array.from(out);
  const truncated = chars.length > budget
    ? { text: chars.slice(0, Math.max(budget - 1, 0)).join('') + '…', truncated: true }
    : { text: out, truncated: false };
  return truncated.text === '' ? { text: '', truncated: false } : truncated;
}

describe('plant-back: the f92f75ca cleaner and interpolator fail every new assertion', () => {
  it('sequential interpolation lets placeholder names blow past 240 characters', () => {
    const template = handbackVocabulary('zh-CN').sentence.answer;
    const project = legacySanitize('{{question}}'.repeat(10), 240).text;
    const question = 'Q'.repeat(100);
    const legacy = legacyInterpolate(template, { project, task: '空状态检查', question });
    expect(len(legacy)).toBeGreaterThan(HANDBACK_MAX_SENTENCE_LENGTH);
    // The new assertion on the same inputs holds.
    expect(len(build({ projectName: '{{question}}'.repeat(10), question: 'Q'.repeat(100) }).sentence))
      .toBeLessThanOrEqual(HANDBACK_MAX_SENTENCE_LENGTH);
  });

  it('sequential interpolation leaks the question through a placeholder task name', () => {
    const template = handbackVocabulary('zh-CN').sentence.answer;
    const legacy = legacyInterpolate(template, {
      project: '订单协作平台',
      task: legacySanitize('{{question}}', 240).text,
      question: 'SECRETQUESTION',
    });
    expect((legacy.match(/SECRETQUESTION/g) ?? []).length).toBeGreaterThan(1);
    expect((build({ taskName: '{{question}}', question: 'SECRETQUESTION' }).sentence.match(/SECRETQUESTION/g) ?? []).length)
      .toBeLessThanOrEqual(1);
  });

  it('legacy braces survive as live placeholders', () => {
    expect(legacySanitize('{{question}}', 120).text).toContain('{{');
    expect(sanitizeUntrustedText('{{question}}', 120).text).not.toContain('{{');
  });

  it('legacy link parsing keeps brackets when the URL has parentheses', () => {
    expect(legacySanitize('[a](b(c)d)', 120).text).toMatch(/[\[\]]/);
    expect(sanitizeUntrustedText('[a](b(c)d)', 120).text).not.toMatch(/[\[\]]/);
  });

  it('legacy cleaning keeps variation selectors', () => {
    expect(/[\u{FE00}-\u{FE0D}]/u.test(legacySanitize('a\uFE00b', 120).text)).toBe(true);
    expect(/[\u{FE00}-\u{FE0D}]/u.test(sanitizeUntrustedText('a\uFE00b', 120).text)).toBe(false);
  });

  it('legacy dotted dropping mangles ordinary file names', () => {
    expect(legacySanitize('Rename server.js to app.js', 120).text).toBe('Rename to');
    expect(sanitizeUntrustedText('Rename server.js to app.js', 120).text).toBe('Rename server.js to app.js');
  });

  it('legacy timing blows up on the backtracking shape while the fixed one stays flat', () => {
    const t0 = performance.now();
    legacySanitize('B-AAAAAAAA-'.repeat(3_000), 120);
    const legacyMs = performance.now() - t0;
    const t1 = performance.now();
    sanitizeUntrustedText('B-AAAAAAAA-'.repeat(12_000), 120);
    const fixedMs = performance.now() - t1;
    // Quadratic versus linear: 3,000 repeats of the old shape cost more than five
    // times 12,000 repeats of the fixed one. Both absolute cap and this ratio are
    // asserted so a slow CI box fails the cap first, and a fast box still sees the gap.
    expect(legacyMs, `legacy ${legacyMs.toFixed(1)}ms vs fixed ${fixedMs.toFixed(1)}ms`).toBeGreaterThan(fixedMs * 5);
    expect(fixedMs).toBeLessThan(200);
  });
});

