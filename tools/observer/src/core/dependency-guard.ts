export interface CoreDependencyViolation {
  rule: string;
  match: string;
}

function checkModule(path: string, violations: CoreDependencyViolation[]) {
  if (/^(?:react|react-dom|antd|react-i18next)(?:\/|$)/.test(path) || /^@tanstack\/react-query(?:\/|$)/.test(path)) {
    violations.push({ rule: 'ui-import', match: path });
  } else if (/(?:^|\/)(?:pages|api|queries)(?:\/|$)/.test(path)) {
    violations.push({ rule: 'page-or-read-import', match: path });
  }
}

/** Pure source scanner used by the core independence guard and its self-tests. */
export function scanCoreSource(source: string): CoreDependencyViolation[] {
  const violations: CoreDependencyViolation[] = [];
  for (const match of source.matchAll(/\b(?:import|export)\s+(?:type\s+)?(?:[^'"]*?\s+from\s+)?['"]([^'"]+)['"]/g)) {
    checkModule(match[1], violations);
  }
  for (const match of source.matchAll(/\b(?:import|require)\s*\(\s*['"]([^'"]+)['"]\s*\)/g)) {
    checkModule(match[1], violations);
  }
  const rules = [
    ['fetch', /\bfetch\s*\(/g],
    ['fetch', /\bXMLHttpRequest\b/g],
    ['dom', /\b(?:document|window|navigator)\s*\./g],
    ['browser-storage', /\b(?:localStorage|sessionStorage|indexedDB)\b/g],
    ['implicit-clock', /\bDate\s*\.\s*now\s*\(/g],
    ['implicit-clock', /\bnew\s+Date\s*\(\s*\)/g],
    ['implicit-clock', /\bperformance\s*\.\s*now\s*\(/g],
    ['timer', /\b(?:setTimeout|setInterval|requestAnimationFrame)\s*\(/g],
  ] as const;
  for (const [rule, pattern] of rules) {
    for (const match of source.matchAll(pattern)) violations.push({ rule, match: match[0] });
  }
  return violations;
}
