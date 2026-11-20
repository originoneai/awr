export interface ForbiddenIconImport {
  file: string;
  line: number;
  excerpt: string;
}

/*
 * Detector for icon-package imports outside the registry. The pattern is written so
 * that this file's own source never matches it: a violation needs the package name
 * in quotes, preceded by an import/require/from form.
 */
const PATTERN =
  /(?:\bfrom\s*|\bimport\s*\(\s*|\brequire\s*\(\s*|\bimport\s+)['"](lucide-react|@ant-design[/]icons)(?:[/][^'"]*)?['"]/g;

/** Returns one entry per icon-package import found in the source (line numbers are 1-based). */
export function findForbiddenIconImports(source: string, file = '<source>'): ForbiddenIconImport[] {
  const hits: ForbiddenIconImport[] = [];
  PATTERN.lastIndex = 0;
  let match: RegExpExecArray | null;
  while ((match = PATTERN.exec(source))) {
    const line = source.slice(0, match.index).split('\n').length;
    hits.push({ file, line, excerpt: source.split('\n')[line - 1].trim() });
  }
  return hits;
}
