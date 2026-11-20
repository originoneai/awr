import { spawn, type ChildProcess } from 'node:child_process';
import path from 'node:path';
import { expect, test } from '@playwright/test';

// AWR-PPO-066: every expected value below is the prototype's own number, measured on
// panorama.html served from .local/ppo-line/kimi-pack/prototype (selector noted per group).
// The companion pair table e2e/pixel-pairs/shell.json drives the supervisor's probe; this
// spec pins the same numbers into CI so regressions fail without the prototype running.
const root = process.cwd();
let server: ChildProcess | undefined;
let baseUrl = '';

const THEME = {
  light: {
    page: [243, 245, 249],
    hairline: 'rgba(17, 24, 39, 0.08)',
    hairlineStrong: 'rgba(17, 24, 39, 0.16)',
    muted: 'rgb(107, 116, 135)',
    secondary: 'rgb(75, 85, 101)',
    raised: 'rgb(236, 239, 244)',
  },
  dark: {
    page: [11, 13, 20],
    hairline: 'rgba(238, 240, 245, 0.08)',
    hairlineStrong: 'rgba(238, 240, 245, 0.16)',
    muted: 'rgb(122, 130, 150)',
    secondary: 'rgb(163, 169, 184)',
    raised: 'rgb(26, 30, 40)',
  },
} as const;

test.beforeAll(async () => {
  server = spawn(process.execPath, ['server.js', '--demo', '--demo-variant', 'full', '--port', '0'], {
    cwd: root,
    stdio: ['ignore', 'pipe', 'pipe'],
  });
  let output = '';
  baseUrl = await new Promise<string>((resolve, reject) => {
    const timeout = setTimeout(() => reject(new Error(`Observer did not start: ${output}`)), 10_000);
    server?.stdout?.on('data', (chunk) => {
      output += String(chunk);
      const match = output.match(/URL\s+(http:\/\/127\.0\.0\.1:\d+)/);
      if (match) {
        clearTimeout(timeout);
        resolve(match[1]);
      }
    });
    server?.stderr?.on('data', (chunk) => { output += String(chunk); });
    server?.once('exit', (code) => {
      clearTimeout(timeout);
      reject(new Error(`Observer exited (${code}): ${output}`));
    });
  });
});

test.afterAll(async () => {
  if (!server || server.killed) return;
  const stopped = new Promise((resolve) => server?.once('exit', resolve));
  server.kill('SIGTERM');
  await stopped;
});

test.beforeEach(async ({ page }, testInfo) => {
  const mode = testInfo.project.name === 'desktop-dark' ? 'dark' : 'light';
  await page.addInitScript((themeMode) => {
    window.localStorage.setItem('awr-observer-theme', themeMode);
    window.localStorage.setItem('i18nextLng', 'zh-CN');
  }, mode);
});

test('the shell matches the prototype numbers at 1440x900', async ({ page }, testInfo) => {
  const mode = testInfo.project.name === 'desktop-dark' ? 'dark' : 'light';
  const palette = THEME[mode];
  await page.goto(`${baseUrl}/activity?project=P-001&kind=development`);
  await page.locator('[data-testid="page-activity"]').waitFor();
  await page.evaluate(() => document.fonts.ready);
  await page.waitForTimeout(300);

  const metrics = await page.evaluate(() => {
    const box = (el: Element | null) => {
      if (!el) return null;
      const r = el.getBoundingClientRect();
      return { x: Math.round(r.left * 100) / 100, y: Math.round(r.top * 100) / 100, w: Math.round(r.width * 100) / 100, h: Math.round(r.height * 100) / 100 };
    };
    const style = (el: Element | null) => (el ? getComputedStyle(el) : null);
    const q = (selector: string) => document.querySelector(selector);
    const search = q('.shell-header > button:nth-of-type(1)');
    const searchIcon = search?.querySelector('span.ant-btn-icon svg') ?? search?.querySelector('svg');
    const searchChildren = search ? [...search.children].filter((n) => n.tagName !== 'STYLE') : [];
    const navItem = (group: number, item: number) => q(`.shell-sider nav:nth-of-type(${group}) li.ant-menu-item:nth-child(${item})`);
    return {
      sider: box(q('.shell-sider')),
      siderPadding: style(q('.shell-sider'))?.padding,
      brand: box(q('.shell-brand')),
      brandMarginBottom: style(q('.shell-brand'))?.marginBottom,
      wordmark: box(q('[data-testid="brand-wordmark"]')),
      wordmarkFont: (() => { const s = style(q('[data-testid="brand-wordmark"]')); return s && { size: s.fontSize, weight: s.fontWeight, lineHeight: s.lineHeight }; })(),
      nav1: box(q('.shell-sider nav:nth-of-type(1) ul.ant-menu')),
      nav1Item1: box(navItem(1, 1)),
      nav1Item1Icon: box(navItem(1, 1)?.querySelector('svg')),
      nav1Item1Label: box(navItem(1, 1)?.querySelector('.ant-menu-title-content a span')),
      nav1Item1LabelStyle: (() => { const s = style(navItem(1, 1)?.querySelector('.ant-menu-title-content a span')); return s && { lineHeight: s.lineHeight, textAlign: s.textAlign }; })(),
      nav2: box(q('.shell-sider nav:nth-of-type(2) ul.ant-menu')),
      nav2Item2: box(navItem(2, 2)),
      nav2Item2Icon: box(navItem(2, 2)?.querySelector('svg')),
      nav2Item2Label: box(navItem(2, 2)?.querySelector('.ant-menu-title-content a span')),
      themeButton: box(q('.shell-theme button')),
      themeIcon: box(q('.shell-theme button svg')),
      themeLabel: box(q('.shell-theme-label')),
      themeStyle: (() => { const s = style(q('.shell-theme button')); return s && { gap: s.columnGap, padding: s.padding, border: s.borderTopWidth, lineHeight: s.lineHeight, whiteSpace: s.whiteSpace, textAlign: s.textAlign }; })(),
      topbar: box(q('.shell-header')),
      topbarStyle: (() => { const s = style(q('.shell-header')); return s && { padding: s.padding, backgroundColor: s.backgroundColor, backdropFilter: s.backdropFilter, borderBottom: s.borderBottomColor }; })(),
      crumbBuildingIcon: box(q('.shell-header nav.ant-breadcrumb svg:first-of-type')),
      crumbSeparator: box(q('.shell-header nav.ant-breadcrumb li.ant-breadcrumb-separator svg')),
      crumbCurrent: box(q('.shell-header nav.ant-breadcrumb li:last-child .ant-breadcrumb-link span')),
      crumbCurrentLineHeight: style(q('.shell-header nav.ant-breadcrumb li:last-child .ant-breadcrumb-link span'))?.lineHeight,
      search: box(search),
      searchStyle: (() => { const s = style(search); return s && { padding: s.padding, borderTopColor: s.borderTopColor, boxShadow: s.boxShadow, color: s.color, fontSize: s.fontSize, textAlign: s.textAlign, whiteSpace: s.whiteSpace, marginLeft: s.marginLeft }; })(),
      searchIcon: box(searchIcon),
      searchLabel: box(searchChildren[1]),
      searchLabelStyle: (() => { const s = style(searchChildren[1]); return s && { fontSize: s.fontSize, lineHeight: s.lineHeight, color: s.color, whiteSpace: s.whiteSpace }; })(),
      searchShortcut: box(searchChildren[2]),
      searchShortcutStyle: (() => { const s = style(searchChildren[2]); return s && { height: s.height, color: s.color, borderColor: s.borderTopColor }; })(),
      bellButton: box(q('.shell-header .ant-badge button')),
      bellIcon: box(q('.shell-header .ant-badge button svg')),
      bellColor: style(q('.shell-header .ant-badge button'))?.color,
      avatar: box(q('[data-testid="current-member-avatar"]')),
      avatarStyle: (() => { const s = style(q('[data-testid="current-member-avatar"]')); return s && { backgroundColor: s.backgroundColor, color: s.color, borderRadius: s.borderTopLeftRadius }; })(),
      avatarString: (() => { const s = style(q('[data-testid="current-member-avatar"] .ant-avatar-string')); return s && { fontSize: s.fontSize, fontWeight: s.fontWeight, lineHeight: s.lineHeight, whiteSpace: s.whiteSpace, textAlign: s.textAlign }; })(),
      content: box(q('.shell-content')),
      contentStyle: (() => { const s = style(q('.shell-content')); return s && { padding: s.padding, maxWidth: s.maxWidth }; })(),
      pageTitle: box(q('.obs-page-title')),
      pageTitleStyle: (() => { const s = style(q('.obs-page-title')); return s && { fontSize: s.fontSize, lineHeight: s.lineHeight, fontWeight: s.fontWeight }; })(),
      mutedVar: style(document.documentElement).getPropertyValue('--craft-color-muted').trim(),
    };
  });

  // --- Sidebar (prototype .app grid 232px; .side padding 16px 12px) ---
  expect(metrics.sider).toMatchObject({ x: 0, y: 0, w: 232, h: 900 });
  expect(metrics.siderPadding).toBe('16px 12px');

  // --- Brand row (prototype .brand: height 32, padding 0 12px, margin-bottom 20px) ---
  expect(metrics.brand).toMatchObject({ x: 12, y: 16, w: 207, h: 32 });
  expect(metrics.brandMarginBottom).toBe('20px');
  // Prototype .brand b: 17px/600 on the body 20px line box. The wordmark sits 2px lower
  // (x 58 vs 56) because the owner's mark is 24px wide where the prototype glyph is 22px.
  expect(metrics.wordmark).toMatchObject({ y: 22, h: 20 });
  expect(metrics.wordmarkFont).toEqual({ size: '17px', weight: '600', lineHeight: '20px' });

  // --- Navigation (prototype .nav: grid gap 2; buttons 36px tall, icon 16px, gap 10px) ---
  expect(metrics.nav1).toMatchObject({ x: 12, y: 68, w: 207, h: 150 });
  expect(metrics.nav1Item1).toMatchObject({ x: 12, y: 68, w: 207, h: 36 });
  expect(metrics.nav1Item1Icon).toMatchObject({ x: 24, y: 78, w: 16, h: 16 });
  expect(metrics.nav1Item1Label).toMatchObject({ x: 50, y: 76, h: 20 });
  expect(metrics.nav1Item1LabelStyle).toEqual({ lineHeight: '20px', textAlign: 'left' });
  // Prototype second group starts after the 24/8 label row: y 266, selected item y 304.
  expect(metrics.nav2).toMatchObject({ x: 12, y: 266, w: 207, h: 112 });
  expect(metrics.nav2Item2).toMatchObject({ x: 12, y: 304, w: 207, h: 36 });
  expect(metrics.nav2Item2Icon).toMatchObject({ x: 24, y: 314, w: 16, h: 16 });
  expect(metrics.nav2Item2Label).toMatchObject({ x: 50, y: 312, h: 20 });

  // --- Theme button (prototype .side-foot button: full row, 36px, icon x 24, label x 50) ---
  // The prototype's own button sits at y 792 because a prototype-only side note follows;
  // without it the row is flush against the sidebar's 16px bottom padding: 900-16-36.
  expect(metrics.themeButton).toMatchObject({ x: 12, w: 207, h: 36, y: expect.any(Number) });
  expect(metrics.themeButton!.y).toBeGreaterThan(847);
  expect(metrics.themeIcon).toMatchObject({ x: 24, w: 16, h: 16 });
  expect(metrics.themeLabel?.x).toBe(50);
  expect(metrics.themeStyle).toMatchObject({ gap: '10px', padding: '0px 12px', border: '0px', lineHeight: '20px', whiteSpace: 'normal', textAlign: 'left' });

  // --- Topbar (prototype .top: height 56, padding 0 32px, page tone at 88% + 8px blur) ---
  expect(metrics.topbar).toMatchObject({ x: 232, y: 0, w: 1208, h: 56 });
  expect(metrics.topbarStyle?.padding).toBe('0px 32px');
  expect(metrics.topbarStyle?.backdropFilter).toBe('blur(8px)');
  expect(metrics.topbarStyle?.borderBottom).toBe(palette.hairline);
  const topMatch = /color\(srgb ([\d.]+) ([\d.]+) ([\d.]+) \/ ([\d.]+)\)/.exec(metrics.topbarStyle?.backgroundColor ?? '');
  expect(topMatch, `topbar background: ${metrics.topbarStyle?.backgroundColor}`).not.toBeNull();
  expect(Number(topMatch![4])).toBeCloseTo(0.88, 2);
  for (const [index, channel] of palette.page.entries()) {
    expect(Number(topMatch![index + 1]) * 255).toBeCloseTo(channel, 0);
  }

  // --- Breadcrumb (prototype .crumb 13px/20px; building icon 16; chevron 14 via icon('chev', 14)) ---
  expect(metrics.crumbBuildingIcon).toMatchObject({ x: 264, w: 16, h: 16 });
  expect(metrics.crumbSeparator).toMatchObject({ w: 14, h: 14, y: 20.5 });
  expect(metrics.crumbCurrentLineHeight).toBe('20px');
  // The current-page and separator x depend on the scope wording width (data-driven).

  // --- Search (prototype button.search: 200x32, padding 0 8px 0 12px, gap 8, hairline, no shadow) ---
  expect(metrics.search).toMatchObject({ w: 200, h: 32, y: 11.5 });
  expect(metrics.searchStyle).toMatchObject({
    padding: '0px 8px 0px 12px',
    borderTopColor: palette.hairline,
    boxShadow: 'none',
    color: palette.muted,
    fontSize: '13px',
    textAlign: 'left',
    whiteSpace: 'normal',
  });
  expect(metrics.searchLabelStyle).toEqual({ fontSize: '13px', lineHeight: '20px', color: palette.muted, whiteSpace: 'normal' });
  expect(metrics.searchIcon).toMatchObject({ y: 19.5, w: 16, h: 16 });
  // 8px from icon right edge to label, and label to shortcut (prototype .search gap 8px).
  expect(metrics.searchLabel!.x - (metrics.searchIcon!.x + metrics.searchIcon!.w)).toBe(8);
  expect(metrics.searchShortcut!.x - (metrics.searchLabel!.x + metrics.searchLabel!.w)).toBeCloseTo(8, 1);
  expect(metrics.searchShortcutStyle).toMatchObject({ height: '20px', color: palette.muted, borderColor: palette.hairlineStrong });

  // --- Bell (prototype .bell 32x32, quiet ink-2; icon('bell', 18)) ---
  expect(metrics.bellButton).toMatchObject({ x: 1332, y: 11.5, w: 32, h: 32 });
  expect(metrics.bellIcon).toMatchObject({ x: 1339, y: 18.5, w: 18, h: 18 });
  expect(metrics.bellColor).toBe(palette.secondary);

  // --- Avatar (prototype .av: 28px circle, raised fill, ink-2 12px/500 initial) ---
  expect(metrics.avatar).toMatchObject({ x: 1380, y: 13.5, w: 28, h: 28 });
  expect(metrics.avatarStyle).toMatchObject({ backgroundColor: palette.raised, color: palette.secondary, borderRadius: '50%' });
  expect(metrics.avatarString).toEqual({ fontSize: '12px', fontWeight: '500', lineHeight: '20px', whiteSpace: 'normal', textAlign: 'start' });

  // --- Content wrapper (prototype .wrap: padding 0 32px 64px, max-width 1480px) ---
  expect(metrics.content).toMatchObject({ x: 232, y: 56, w: 1208 });
  expect(metrics.contentStyle).toEqual({ padding: '0px 32px 64px', maxWidth: '1480px' });

  // --- Page title (prototype .head padding-top 24 + h1 22px/28px) ---
  expect(metrics.pageTitle).toMatchObject({ x: 264, y: 80, h: 28 });
  expect(metrics.pageTitleStyle).toEqual({ fontSize: '22px', lineHeight: '28px', fontWeight: '500' });

  // --- Weak ink token restored to the prototype --ink-3 ---
  expect(metrics.mutedVar).toBe(mode === 'light' ? '#6B7487' : '#7A8296');
});
