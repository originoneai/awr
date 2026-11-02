#!/usr/bin/env node
// Public, deterministic source audit. Markers follow semantics, never private data or fixed line numbers.
import fs from 'node:fs';
import path from 'node:path';

const ROOT = process.cwd();
const BASELINE = 'src/test-fixtures/derived-duplication-baseline.json';
const range = (file, start, end, slice, after) => ({ file, start, end: end ?? start, slice, ...(after ? { after } : {}) });
export const RULES = [
  { id: 'raw-scalar', ranges: [
    range('src/pages/ProjectOverview.tsx', 'function rawText(', '}', 'work-state'),
    range('src/pages/WorkPlanPage.tsx', 'function rawText(', '}', 'work-state'),
    range('src/components/WorkDetailPanel.tsx', 'function rawText(', '}', 'work-state'),
    range('src/pages/ActivityPage.tsx', 'function raw(', '}', 'fold-operations'),
    range('src/pages/ProjectKnowledge.tsx', 'function textOf(', '}', 'page-adoption'),
  ] },
  { id: 'last-report', ranges: [
    range('src/pages/WorkbenchPage.tsx', 'const { t, i18n } = useTranslation();', ': formatUnixMs(clientReportedAt, i18n.language);', 'work-state', 'function LastReport'),
    range('src/components/WorkDetailPanel.tsx', 'const reportedAt =', undefined, 'work-state'),
    range('src/components/WorkDetailPanel.tsx', "key: 'report'", '</span>', 'work-state'),
  ] },
  { id: 'next-guidance', ranges: [
    range('src/pages/WorkbenchPage.tsx', 'const progress =', 'const nextStep =', 'work-state'),
    range('src/components/WorkDetailPanel.tsx', 'const guidance =', undefined, 'work-state'),
    range('src/components/WorkDetailPanel.tsx', "{guidance?.op ?? t('common.notProvided')}", undefined, 'work-state'),
  ] },
  { id: 'workstream-grouping', ranges: [
    range('src/pages/WorkPlanPage.tsx', 'function groupedWorks(', '^}', 'work-state'),
  ] },
  { id: 'activity-ordering', ranges: [
    range('src/components/WorkDetailPanel.tsx', 'const latest = [...items]', '.slice(0, 20);', 'fold-operations'),
    range('src/pages/ActivityPage.tsx', 'const items = useMemo(() => {', '}, [activity.data]);', 'fold-operations'),
  ] },
  { id: 'claimed-status', ranges: [
    range('src/pages/WorkbenchPage.tsx', '? <Tag color={row.state', '</Tag>', 'work-state', 'function WorkDetail('),
    range('src/pages/WorkbenchPage.tsx', '? <Tag color={row.state', '</Tag>', 'work-state', 'export function WorkbenchPage'),
  ] },
  { id: 'activity-raw', ranges: [
    range('src/pages/ActivityPage.tsx', 'description={<span data-testid="activity-unknown-result">', undefined, 'fold-operations'),
    range('src/pages/ActivityPage.tsx', 'request_retention:', undefined, 'fold-operations'),
    range('src/pages/ActivityPage.tsx', '{firstPage?.request_retention_capacity', undefined, 'fold-operations'),
    range('src/pages/ActivityPage.tsx', 'development_source:', undefined, 'fold-operations'),
    range('src/pages/ActivityPage.tsx', 'chat_text_collected:', undefined, 'fold-operations'),
    range('src/pages/ActivityPage.tsx', "{' · '}tool_io_collected:", undefined, 'fold-operations'),
    range('src/pages/ActivityPage.tsx', 'unknown_means:', undefined, 'fold-operations'),
  ] },
  { id: 'time-wrapper', ranges: [
    range('src/pages/ProjectAcceptance.tsx', 'const timeValue =', 'if (raw == null', 'page-adoption'),
  ] },
  { id: 'knowledge-digest', ranges: [
    range('src/pages/ProjectKnowledge.tsx', '/** UTF-8 byte length', '}', 'page-adoption'),
    range('src/pages/ProjectKnowledge.tsx', 'if (!selectedText', '^}', 'page-adoption'),
    range('src/pages/ProjectKnowledge.tsx', "if (typeof crypto === 'undefined'", '^}', 'page-adoption'),
    range('src/pages/ProjectKnowledge.tsx', "crypto.subtle.digest('SHA-256'", "setDigestCheck(hex === selectedSha256", 'page-adoption'),
  ] },
  { id: 'digest-label', ranges: [
    range('src/pages/ProjectKnowledge.tsx', "{digestCheck === 'verified'", ": t('knowledge.digestUnverified')}", 'page-adoption', "aria-label={t('knowledge.digestVerified')}"),
  ] },
];

export function countRange(source, rule) {
  const lines = source.split('\n');
  const afterIndex = rule.after ? lines.findIndex((line) => line.includes(rule.after)) : -1;
  if (rule.after && afterIndex < 0) return 0;
  const start = lines.findIndex((line, index) => index > afterIndex && line.includes(rule.start));
  if (start < 0) return 0;
  const end = lines.findIndex((line, index) => index >= start && (rule.end === '^}' ? line.trim() === '}' : line.includes(rule.end)));
  return end < 0 ? 0 : end - start + 1;
}

export function measureDuplication(root = ROOT) {
  const rules = {};
  const slices = { 'fold-operations': 0, 'work-state': 0, 'page-adoption': 0 };
  for (const { id, ranges } of RULES) {
    let count = 0;
    for (const rule of ranges) {
      const source = fs.readFileSync(path.join(root, rule.file), 'utf8');
      const lines = countRange(source, rule);
      count += lines;
      slices[rule.slice] += lines;
    }
    rules[id] = count;
  }
  return { total: Object.values(rules).reduce((sum, count) => sum + count, 0), target: 0, slices, rules };
}

function main() {
  if (process.argv.includes('--manifest')) {
    console.log(JSON.stringify(RULES, null, 2));
    return;
  }
  const actual = measureDuplication();
  if (process.argv.includes('--update')) {
    fs.writeFileSync(BASELINE, `${JSON.stringify(actual, null, 2)}\n`);
  }
  if (process.argv.includes('--json')) {
    console.log(JSON.stringify(actual, null, 2));
    return;
  }
  const baseline = JSON.parse(fs.readFileSync(BASELINE, 'utf8'));
  console.log(JSON.stringify({ actual, baseline }, null, 2));
  if (JSON.stringify(actual) !== JSON.stringify(baseline)) {
    console.error('Derived-duplication baseline changed; run with --update after an intentional adoption slice.');
    process.exitCode = 1;
  }
}

if (process.argv[1] && import.meta.url === new URL(`file://${path.resolve(process.argv[1])}`, 'file:').href) main();
