export interface CoreDependencyViolation {
  rule: string;
  match: string;
}

/** Pure source scanner used by the core independence guard and its self-tests. */
export function scanCoreSource(source: string): CoreDependencyViolation[] {
  const violations: CoreDependencyViolation[] = [];
  for (const match of source.matchAll(/\b(?:import|export)\s+(?:type\s+)?(?:[^'"]*?\s+from\s+)?['"]([^'"]+)['"]/g)) {
    const path = match[1];
    if (/^(?:react|react-dom|antd|react-i18next)(?:\/|$)/.test(path) || path === '@tanstack/react-query') {
      violations.push({ rule: 'ui-import', match: path });
    } else if (/(?:^|\/)(?:pages|api|queries)(?:\/|$)/.test(path)) {
      violations.push({ rule: 'page-or-read-import', match: path });
    }
  }
  const rules = [
    ['fetch', /\bfetch\s*\(/g],
    ['dom', /\b(?:document|window|navigator)\s*\./g],
    ['implicit-clock', /\bDate\s*\.\s*now\s*\(/g],
  ] as const;
  for (const [rule, pattern] of rules) {
    for (const match of source.matchAll(pattern)) violations.push({ rule, match: match[0] });
  }
  return violations;
}
