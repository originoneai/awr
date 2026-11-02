import { readFileSync, readdirSync } from 'node:fs';
import { describe, expect, it } from 'vitest';

describe('craft layout rhythm', () => {
  it('uses the prototype master/detail outcome layout with a dominant status and evidence region', () => {
    const source = readFileSync('src/pages/ProjectAcceptance.tsx', 'utf8');
    expect(source).toContain('<DeliverySteps');
    expect(source).toContain('testId="acceptance-evidence-chain"');
    expect(source).toContain('lg={7}');
    expect(source).toContain('lg={17}');
    expect(source).toContain('acceptance.verificationTitle');
    expect(source).toContain('acceptance.reviewChanges');
    expect(source).toContain('acceptance.downstream');
  });

  it('uses asymmetric account and connection grids instead of equal card stacks', () => {
    const account = readFileSync('src/pages/AccountPage.tsx', 'utf8');
    const connect = readFileSync('src/pages/AgentConnectPage.tsx', 'utf8');
    expect(account).toContain('<Row gutter={[16, 16]}>');
    expect(account).toContain('lg={14}');
    expect(account).toContain('lg={10}');
    expect(connect).toContain('<Row gutter={[16, 16]}>');
    expect(connect).toContain('lg={14}');
    expect(connect).toContain('lg={10}');
  });

  it('keeps workline progress and recent activity as the main overview blocks', () => {
    const overview = readFileSync('src/pages/ProjectOverview.tsx', 'utf8');
    expect(overview).toContain('<Col xs={24} lg={16} style={{ minWidth: 0 }}>');
    expect(overview).toContain('<RecentActivity query={activity} project={project} />');
  });
});

// ---------------------------------------------------------------------------
// Narrow-viewport guard (AWR-OBS-021): the page must not be forced wider than a 375px phone by
// fixed sizes. jsdom cannot measure layout, so this is a source-level rule: no fixed
// width/min-width of 376px or more in the shell CSS or in inline styles, unless the declaration
// sits inside a media query that does not apply at 375px (a min-width guard) or is allowlisted.
// Deliberately NOT covered: antd Table column widths and scroll={{ x }} — tables scroll inside
// their own card, which the mobile acceptance allows.
// ---------------------------------------------------------------------------

const NARROW_VIEWPORT = 375;

/** A finding is the offending declaration text; the message names file and line. */
function cssFixedWidthsOverNarrowViewport(css: string): string[] {
  const findings: string[] = [];
  const mediaStack: string[] = [];
  css.split('\n').forEach((line, index) => {
    const media = line.match(/^@media([^{]+)\{/);
    if (media) {
      mediaStack.push(media[1].trim());
      return;
    }
    if (/^\}/.test(line.trimStart())) {
      mediaStack.pop();
      return;
    }
    const declaration = line.match(/(?<![-a-zA-Z])(min-)?width:\s*([0-9]+(?:\.[0-9]+)?)px/);
    if (!declaration) return;
    if (Number(declaration[2]) < 376) return;
    // A media query with a min-width above the narrow viewport does not apply at 375px, so the
    // declaration inside it cannot force the phone page wide; other queries do apply.
    const guarded = mediaStack.some((query) => {
      const minWidths = [...query.matchAll(/\(min-width:\s*([0-9.]+)px\)/g)].map((match) => Number(match[1]));
      return minWidths.length > 0 && minWidths.every((px) => px > NARROW_VIEWPORT);
    });
    if (!guarded) findings.push(`line ${index + 1}: ${line.trim()}`);
  });
  return findings;
}

function tsxInlineFixedWidthsOverNarrowViewport(source: string): string[] {
  const findings: string[] = [];
  for (const style of source.matchAll(/style=\{\{([^}]*)\}\}/g)) {
    for (const declaration of style[1].matchAll(/(?<![\w-])(minWidth|width)\s*:\s*(?:'([0-9]+)px'|([0-9]+))/g)) {
      const value = Number(declaration[2] ?? declaration[3]);
      if (value >= 376) findings.push(`inline style: ${declaration[0]}`);
    }
  }
  return findings;
}

// Each entry is a full trimmed declaration line that is allowed to keep a fixed width ≥376px,
// with the reason documented here. Add only with a justification that holds at 375px.
const ALLOWED_FIXED_WIDTH_LINES: Record<string, string[]> = {
  // (none today)
};

const LAYOUT_FILES = [
  'src/global.css',
  ...readdirSync('src/pages').filter((name) => name.endsWith('.tsx')).map((name) => `src/pages/${name}`),
  ...readdirSync('src/components').filter((name) => name.endsWith('.tsx')).map((name) => `src/components/${name}`),
];

describe('narrow viewport guard', () => {
  it.each([
    ['flags a fixed width wider than the phone viewport', '.card { width: 420px; }', ['line 1: .card { width: 420px; }']],
    ['flags a fixed min-width', '.card {\n  min-width: 1200px;\n}', ['line 2: min-width: 1200px;']],
    ['allows widths that fit the phone viewport', '.card { width: 375px; max-width: 1600px; width: 240px; }', []],
    ['allows wide-only declarations inside a min-width media query', '@media (min-width: 992px) {\n  .card { width: 400px; }\n}', []],
    ['flags wide declarations inside a max-width media query (applies at 375px)', '@media (max-width: 768px) {\n  .card { width: 400px; }\n}', ['line 2: .card { width: 400px; }']],
    ['flags wide declarations inside a preference media query (may apply at 375px)', '@media (prefers-reduced-motion: reduce) {\n  .card { width: 400px; }\n}', ['line 2: .card { width: 400px; }']],
  ])('%s', (_name, css, expected) => {
    expect(cssFixedWidthsOverNarrowViewport(css)).toEqual(expected);
  });

  it('allows an allowlisted declaration when it is justified', () => {
    const allowlist = { 'src/global.css': ['.card { width: 400px; }  // wide-only: chart canvas, hidden below lg'] };
    const findings = cssFixedWidthsOverNarrowViewport('.card { width: 400px; }  // wide-only: chart canvas, hidden below lg')
      .filter((line) => !allowlist['src/global.css']?.some((allowed) => line.endsWith(allowed)));
    expect(findings).toEqual([]);
  });

  it.each([
    ['flags a fixed inline width', 'const card = <div style={{ width: 400 }} />', ['inline style: width: 400']],
    ['flags a fixed inline min-width in px', "const card = <div style={{ minWidth: '1200px' }} />", ["inline style: minWidth: '1200px'"]],
    ['allows fluid and narrow inline widths', "const card = <div style={{ width: '100%', maxWidth: 420, minWidth: 0 }} />", []],
    ['ignores table column widths (tables scroll inside their card)', "const columns = [{ title: '时间', dataIndex: 'at', width: 400 }];", []],
    ['ignores scroll ranges (tables scroll inside their card)', 'const table = <Table scroll={{ x: 960 }} />;', []],
  ])('%s', (_name, source, expected) => {
    expect(tsxInlineFixedWidthsOverNarrowViewport(source)).toEqual(expected);
  });

  it('keeps every shell, page and component file free of phone-breaking fixed widths', () => {
    const violations = LAYOUT_FILES.flatMap((file) => {
      const source = readFileSync(file, 'utf8');
      const css = file.endsWith('.css')
        ? cssFixedWidthsOverNarrowViewport(source).filter((finding) => !ALLOWED_FIXED_WIDTH_LINES[file]?.some((allowed) => finding.endsWith(allowed)))
        : [];
      const tsx = file.endsWith('.tsx') ? tsxInlineFixedWidthsOverNarrowViewport(source) : [];
      return [...css, ...tsx].map((finding) => `${file}: ${finding}`);
    });
    expect(violations).toEqual([]);
  });

  it('sizes the mobile navigation entry and drawer rows for touch', () => {
    const css = readFileSync('src/global.css', 'utf8');
    const shell = readFileSync('src/components/AppShell.tsx', 'utf8');
    const theme = readFileSync('src/theme.ts', 'utf8');
    // The entry box comes from the touch-target token as an inline style (antd's icon-only width
    // would otherwise win over a class rule); the drawer rows raise the menu row height to it.
    expect(theme).toMatch(/touchTarget:\s*40/);
    expect(shell).toMatch(/width: craftTokens\.touchTarget, height: craftTokens\.touchTarget/);
    expect(css).toMatch(/\.shell-drawer \.shell-nav \.ant-menu-item\s*\{[^}]*height: 40px;/s);
    expect(css).toMatch(/\.shell-drawer \.ant-drawer-close\s*\{[^}]*width: 40px;[^}]*height: 40px;/s);
  });
});
