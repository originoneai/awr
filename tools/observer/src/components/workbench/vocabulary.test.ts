import { readFileSync } from 'node:fs';
import { describe, expect, it } from 'vitest';
import enUS from '../../i18n/en-US';
import zhCN from '../../i18n/zh-CN';
import { VERDICT_RULES, workbenchVocabulary } from './model';

function flatten(value: unknown, prefix = ''): Record<string, string> {
  if (typeof value === 'string') return { [prefix]: value };
  if (!value || typeof value !== 'object' || Array.isArray(value)) throw new Error(`Invalid vocabulary at ${prefix}`);
  return Object.assign({}, ...Object.entries(value).flatMap(([key, child]) => Object.entries(flatten(child, `${prefix}${key}.`))));
}

describe('workbench vocabulary and verdict rule table', () => {
  it('has identical Chinese and English keys and placeholders', () => {
    const zh = flatten(workbenchVocabulary('zh-CN'));
    const en = flatten(workbenchVocabulary('en-US'));
    expect(Object.keys(en).sort()).toEqual(Object.keys(zh).sort());
    for (const [key, text] of Object.entries(zh)) {
      expect(en[key].match(/\{\{[^}]+\}\}/g)?.sort()).toEqual(text.match(/\{\{[^}]+\}\}/g)?.sort());
    }
    expect(Object.keys(flatten(enUS.workbench)).sort()).toEqual(Object.keys(flatten(zhCN.workbench)).sort());
  });

  it('keeps internal identifiers, hashes, protocol fields and raw status codes out of all workbench copy', () => {
    const forbidden = /\b(?:AWR-[A-Z0-9-]+|protocol_version|workstream_id|contract_hash|source_snapshot_id|coordinator_epoch|execution_authorized|not_measured|[0-9a-f]{16,})\b/i;
    for (const language of ['zh-CN', 'en-US'] as const) {
      for (const [key, text] of Object.entries(flatten(workbenchVocabulary(language)))) {
        expect(forbidden.test(text), `${language} ${key}: ${text}`).toBe(false);
      }
      for (const [key, text] of Object.entries(flatten(language === 'zh-CN' ? zhCN.workbench : enUS.workbench))) {
        expect(forbidden.test(text), `${language} ${key}: ${text}`).toBe(false);
      }
    }
  });

  it('checks every documented rule row against the implementation and both sentence catalogs', () => {
    const documentText = readFileSync('../../docs/dev/reference/workbench-verdict.md', 'utf8');
    const rows = [...documentText.matchAll(/^\| `([^`]+)` \| `([^`]+)` \| `([^`]+)` \|/gm)].map((match) => ({
      id: match[1], selection: match[2], sentenceKey: match[3],
    }));
    expect(rows.length).toBe(VERDICT_RULES.length);
    expect(rows.map((row) => row.id)).toEqual(VERDICT_RULES.map((rule) => rule.id));
    for (const [index, rule] of VERDICT_RULES.entries()) {
      expect(rule.selection).toBe(rows[index].selection);
      expect(rule.sentenceKey).toBe(rows[index].sentenceKey);
      for (const language of ['zh-CN', 'en-US'] as const) {
        const catalog = workbenchVocabulary(language);
        expect(catalog.verdict.sentences[rule.sentenceKey], `${language} ${rule.id}`).toBeTruthy();
      }
    }
  });

  it('proves the table test notices a missing row and a changed sentence key', () => {
    const documentText = readFileSync('../../docs/dev/reference/workbench-verdict.md', 'utf8');
    expect(documentText).toContain('`no-update-health`');
    expect(documentText).toContain('`generated-pending-and-waiting`');
    expect(VERDICT_RULES.map((rule) => rule.id)).not.toContain('invented-health-rule');
  });
});
