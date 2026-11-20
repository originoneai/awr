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
      navIconPaths: [1, 2].flatMap((group) => [...document.querySelectorAll(`.shell-sider nav:nth-of-type(${group}) li.ant-menu-item svg`)].map((svg) => [...svg.children].map((n) => n.tagName === 'circle' ? `circle:${n.getAttribute('cx')},${n.getAttribute('cy')},${n.getAttribute('r')}` : n.getAttribute('d')).join(' | '))),
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
      htmlSmoothing: style(document.documentElement).getPropertyValue('-webkit-font-smoothing'),
      bodySmoothing: style(document.body).getPropertyValue('-webkit-font-smoothing'),
      bodyFont: (() => { const s = style(document.body); return { family: s.fontFamily, size: s.fontSize, lineHeight: s.lineHeight, weight: s.fontWeight }; })(),
      shortcutFamily: style(searchChildren[2])?.fontFamily,
    };
  });

  // --- Sidebar (prototype .app grid 232px; .side padding 16px 12px) ---
  expect.soft(metrics.sider).toMatchObject({ x: 0, y: 0, w: 232, h: 900 });
  expect.soft(metrics.siderPadding).toBe('16px 12px');

  // --- Brand row (prototype .brand: height 32, padding 0 12px, margin-bottom 20px) ---
  expect.soft(metrics.brand).toMatchObject({ x: 12, y: 16, w: 207, h: 32 });
  expect.soft(metrics.brandMarginBottom).toBe('20px');
  // Prototype .brand b: 17px/600 on the body 20px line box. The wordmark sits 2px lower
  // (x 58 vs 56) because the owner's mark is 24px wide where the prototype glyph is 22px.
  expect.soft(metrics.wordmark).toMatchObject({ y: 22, h: 20 });
  expect.soft(metrics.wordmarkFont).toEqual({ size: '17px', weight: '600', lineHeight: '20px' });

  // --- Navigation (prototype .nav: grid gap 2; buttons 36px tall, icon 16px, gap 10px) ---
  expect.soft(metrics.nav1).toMatchObject({ x: 12, y: 68, w: 207, h: 150 });
  expect.soft(metrics.nav1Item1).toMatchObject({ x: 12, y: 68, w: 207, h: 36 });
  expect.soft(metrics.nav1Item1Icon).toMatchObject({ x: 24, y: 78, w: 16, h: 16 });
  expect.soft(metrics.nav1Item1Label).toMatchObject({ x: 50, y: 76, h: 20 });
  expect.soft(metrics.nav1Item1LabelStyle).toEqual({ lineHeight: '20px', textAlign: 'left' });
  // Prototype second group starts after the 24/8 label row: y 266, selected item y 304.
  expect.soft(metrics.nav2).toMatchObject({ x: 12, y: 266, w: 207, h: 112 });
  expect.soft(metrics.nav2Item2).toMatchObject({ x: 12, y: 304, w: 207, h: 36 });
  expect.soft(metrics.nav2Item2Icon).toMatchObject({ x: 24, y: 314, w: 16, h: 16 });
  expect.soft(metrics.nav2Item2Label).toMatchObject({ x: 50, y: 312, h: 20 });
  // Prototype ICONS (app.js), in sidebar order: home, folder, inbox, book / users, activity, sliders.
  // lucide redesigned eight of these glyphs after the prototype's snapshot; the app rebuilds
  // the prototype path data through lucide's createLucideIcon (src/components/ShellGlyphs.tsx).
  expect.soft(metrics.navIconPaths).toEqual([
    'M3 10.5 12 3l9 7.5 | M5 9.5V20a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1V9.5',
    'M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z',
    'M22 12h-6l-2 3h-4l-2-3H2 | M5.45 5.11 2 12v6a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2v-6l-3.45-6.89A2 2 0 0 0 16.76 4H7.24a2 2 0 0 0-1.79 1.11Z',
    'M12 7v14 | M3 18a1 1 0 0 1-1-1V4a1 1 0 0 1 1-1h5a4 4 0 0 1 4 4 4 4 0 0 1 4-4h5a1 1 0 0 1 1 1v13a1 1 0 0 1-1 1h-6a3 3 0 0 0-3 3 3 3 0 0 0-3-3Z',
    'M16 21v-2a4 4 0 0 0-4-4H6a4 4 0 0 0-4 4v2 | circle:9,7,4 | M22 21v-2a4 4 0 0 0-3-3.87 | M16 3.13a4 4 0 0 1 0 7.75',
    'M22 12h-4l-3 9L9 3l-3 9H2',
    'M4 6h8 | M16 6h4 | M4 12h2 | M10 12h10 | M4 18h10 | M18 18h2 | circle:14,6,2 | circle:8,12,2 | circle:16,18,2',
  ]);

  // --- Theme button (prototype .side-foot button: full row, 36px, icon x 24, label x 50) ---
  // The prototype's own button sits at y 792 because a prototype-only side note follows;
  // without it the row is flush against the sidebar's 16px bottom padding: 900-16-36.
  expect.soft(metrics.themeButton).toMatchObject({ x: 12, w: 207, h: 36, y: expect.any(Number) });
  expect.soft(metrics.themeButton!.y).toBeGreaterThan(847);
  expect.soft(metrics.themeIcon).toMatchObject({ x: 24, w: 16, h: 16 });
  expect.soft(metrics.themeLabel?.x).toBe(50);
  expect.soft(metrics.themeStyle).toMatchObject({ gap: '10px', padding: '0px 12px', border: '0px', lineHeight: '20px', whiteSpace: 'normal', textAlign: 'left' });

  // --- Topbar (prototype .top: height 56, padding 0 32px, page tone at 88% + 8px blur) ---
  expect.soft(metrics.topbar).toMatchObject({ x: 232, y: 0, w: 1208, h: 56 });
  expect.soft(metrics.topbarStyle?.padding).toBe('0px 32px');
  expect.soft(metrics.topbarStyle?.backdropFilter).toBe('blur(8px)');
  expect.soft(metrics.topbarStyle?.borderBottom).toBe(palette.hairline);
  const topMatch = /color\(srgb ([\d.]+) ([\d.]+) ([\d.]+) \/ ([\d.]+)\)/.exec(metrics.topbarStyle?.backgroundColor ?? '');
  expect.soft(topMatch, `topbar background: ${metrics.topbarStyle?.backgroundColor}`).not.toBeNull();
  expect.soft(Number(topMatch![4])).toBeCloseTo(0.88, 2);
  for (const [index, channel] of palette.page.entries()) {
    expect.soft(Number(topMatch![index + 1]) * 255).toBeCloseTo(channel, 0);
  }

  // --- Breadcrumb (prototype .crumb 13px/20px; building icon 16; chevron 14 via icon('chev', 14)) ---
  expect.soft(metrics.crumbBuildingIcon).toMatchObject({ x: 264, w: 16, h: 16 });
  expect.soft(metrics.crumbSeparator).toMatchObject({ w: 14, h: 14, y: 20.5 });
  expect.soft(metrics.crumbCurrentLineHeight).toBe('20px');
  // The current-page and separator x depend on the scope wording width (data-driven).

  // --- Search (prototype button.search: 200x32, padding 0 8px 0 12px, gap 8, hairline, no shadow) ---
  expect.soft(metrics.search).toMatchObject({ w: 200, h: 32, y: 11.5 });
  expect.soft(metrics.searchStyle).toMatchObject({
    padding: '0px 8px 0px 12px',
    borderTopColor: palette.hairline,
    boxShadow: 'none',
    color: palette.muted,
    fontSize: '13px',
    textAlign: 'left',
    whiteSpace: 'normal',
  });
  expect.soft(metrics.searchLabelStyle).toEqual({ fontSize: '13px', lineHeight: '20px', color: palette.muted, whiteSpace: 'normal' });
  expect.soft(metrics.searchIcon).toMatchObject({ y: 19.5, w: 16, h: 16 });
  // 8px from icon right edge to label, and label to shortcut (prototype .search gap 8px).
  expect.soft(metrics.searchLabel!.x - (metrics.searchIcon!.x + metrics.searchIcon!.w)).toBe(8);
  expect.soft(metrics.searchShortcut!.x - (metrics.searchLabel!.x + metrics.searchLabel!.w)).toBeCloseTo(8, 1);
  expect.soft(metrics.searchShortcutStyle).toMatchObject({ height: '20px', color: palette.muted, borderColor: palette.hairlineStrong });
  // Prototype --mono, verbatim (including Menlo) for the shortcut key cap. Chrome's
  // computed serialization drops the quotes around the unambiguous SFMono-Regular.
  expect.soft(metrics.shortcutFamily).toBe('ui-monospace, SFMono-Regular, "Cascadia Mono", "Roboto Mono", Menlo, monospace');

  // --- Body type (prototype body: 400 14px/20px var(--font), antialiased) ---
  expect.soft(metrics.htmlSmoothing).toBe('antialiased');
  expect.soft(metrics.bodySmoothing).toBe('antialiased');
  expect.soft(metrics.bodyFont).toEqual({
    // Chrome's computed serialization renames the bare BlinkMacSystemFont to "system-ui";
    // both pages compute identically (the probe compares them directly and passes).
    family: '-apple-system, "system-ui", "Segoe UI", "PingFang SC", "Hiragino Sans GB", "Noto Sans SC", "Microsoft YaHei", sans-serif',
    size: '14px',
    lineHeight: '20px',
    weight: '400',
  });

  // --- Bell (prototype .bell 32x32, quiet ink-2; icon('bell', 18)) ---
  expect.soft(metrics.bellButton).toMatchObject({ x: 1332, y: 11.5, w: 32, h: 32 });
  expect.soft(metrics.bellIcon).toMatchObject({ x: 1339, y: 18.5, w: 18, h: 18 });
  expect.soft(metrics.bellColor).toBe(palette.secondary);

  // --- Avatar (prototype .av: 28px circle, raised fill, ink-2 12px/500 initial) ---
  expect.soft(metrics.avatar).toMatchObject({ x: 1380, y: 13.5, w: 28, h: 28 });
  expect.soft(metrics.avatarStyle).toMatchObject({ backgroundColor: palette.raised, color: palette.secondary, borderRadius: '50%' });
  expect.soft(metrics.avatarString).toEqual({ fontSize: '12px', fontWeight: '500', lineHeight: '20px', whiteSpace: 'normal', textAlign: 'start' });

  // --- Content wrapper (prototype .wrap: padding 0 32px 64px, max-width 1480px) ---
  expect.soft(metrics.content).toMatchObject({ x: 232, y: 56, w: 1208 });
  expect.soft(metrics.contentStyle).toEqual({ padding: '0px 32px 64px', maxWidth: '1480px' });

  // --- Page title (prototype .head padding-top 24 + h1 22px/28px) ---
  expect.soft(metrics.pageTitle).toMatchObject({ x: 264, y: 80, h: 28 });
  expect.soft(metrics.pageTitleStyle).toEqual({ fontSize: '22px', lineHeight: '28px', fontWeight: '500' });

  // --- Weak ink token restored to the prototype --ink-3 ---
  expect.soft(metrics.mutedVar).toBe(mode === 'light' ? '#6B7487' : '#7A8296');
});

test('the shell matches the prototype numbers at 390x844', async ({ page }, testInfo) => {
  const mode = testInfo.project.name === 'desktop-dark' ? 'dark' : 'light';
  const palette = THEME[mode];
  const ink = mode === 'light' ? 'rgb(17, 24, 39)' : 'rgb(238, 240, 245)';
  const side = mode === 'light' ? 'rgb(255, 255, 255)' : 'rgb(7, 8, 13)';
  const surface = mode === 'light' ? 'rgb(255, 255, 255)' : 'rgb(17, 20, 28)';
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(`${baseUrl}/activity?project=P-001&kind=development`);
  await page.locator('[data-testid="page-activity"]').waitFor();
  await page.evaluate(() => document.fonts.ready);
  await page.waitForTimeout(300);

  const metrics = await page.evaluate(() => {
    const q = (selector: string) => document.querySelector(selector);
    const box = (el: Element | null) => {
      if (!el) return null;
      const r = el.getBoundingClientRect();
      return { x: Math.round(r.left * 100) / 100, y: Math.round(r.top * 100) / 100, w: Math.round(r.width * 100) / 100, h: Math.round(r.height * 100) / 100 };
    };
    const style = (el: Element | null) => (el ? getComputedStyle(el) : null);
    const navigation = q('[data-testid="mobile-navigation"]');
    const brand = q('.shell-brand-mobile');
    const mark = [...document.querySelectorAll('.shell-brand-mobile .shell-brand-mark')].find((el) => el.getBoundingClientRect().width > 0) ?? null;
    const wordmark = q('.shell-brand-wordmark');
    const selector = q('[data-testid="page-selector"]');
    const theme = q('[data-testid="mobile-theme-toggle"]');
    const header = q('.shell-header');
    const content = q('.shell-content');
    const pageHeader = q('.obs-page-header');
    const title = q('.obs-page-title');
    return {
      navigation: box(navigation),
      navigationStyle: (() => { const s = style(navigation); return s && { padding: s.padding, lineHeight: s.lineHeight, background: s.backgroundColor, border: s.borderBottomColor, position: s.position }; })(),
      brand: box(brand),
      brandStyle: (() => { const s = style(brand); return s && { padding: s.padding, margin: s.margin, gap: s.columnGap, lineHeight: s.lineHeight }; })(),
      mark: box(mark),
      wordmark: box(wordmark),
      wordmarkStyle: (() => { const s = style(wordmark); return s && { size: s.fontSize, lineHeight: s.lineHeight, weight: s.fontWeight, color: s.color }; })(),
      selector: box(selector),
      selectorStyle: (() => { const s = style(selector); return s && { padding: s.padding, size: s.fontSize, lineHeight: s.lineHeight, color: s.color, background: s.backgroundColor, border: s.borderTopColor, radius: s.borderTopLeftRadius }; })(),
      theme: box(theme),
      themeStyle: (() => { const s = style(theme); return s && { padding: s.padding, gap: s.columnGap, borderWidth: s.borderTopWidth, lineHeight: s.lineHeight, whiteSpace: s.whiteSpace, textAlign: s.textAlign, color: s.color, background: s.backgroundColor }; })(),
      themeIcon: box(theme?.querySelector('svg')),
      themeIconColor: style(theme?.querySelector('svg'))?.color,
      header: box(header),
      headerStyle: (() => { const s = style(header); return s && { padding: s.padding, gap: s.columnGap, size: s.fontSize, lineHeight: s.lineHeight, color: s.color, textAlign: s.textAlign, position: s.position, border: s.borderBottomColor, background: s.backgroundColor, backdrop: s.backdropFilter }; })(),
      scopeStyle: (() => { const s = style(q('.shell-header .shell-scope')); return s && { size: s.fontSize, lineHeight: s.lineHeight, color: s.color, textAlign: s.textAlign }; })(),
      buildingIcon: box(q('.shell-header nav.ant-breadcrumb svg:first-of-type')),
      currentCrumb: box(q('.shell-header nav.ant-breadcrumb li:last-child .ant-breadcrumb-link span')),
      currentCrumbStyle: (() => { const s = style(q('.shell-header nav.ant-breadcrumb li:last-child .ant-breadcrumb-link span')); return s && { size: s.fontSize, lineHeight: s.lineHeight, weight: s.fontWeight, color: s.color }; })(),
      separator: box(q('.shell-header nav.ant-breadcrumb li.ant-breadcrumb-separator svg')),
      avatarDisplay: style(q('[data-testid="avatar-menu-trigger"]'))?.display,
      content: box(content),
      contentStyle: (() => { const s = style(content); return s && { padding: s.padding, maxWidth: s.maxWidth }; })(),
      pageHeader: box(pageHeader),
      pageHeaderPadding: style(pageHeader)?.padding,
      title: box(title),
    };
  });

  // Prototype @media(max-width:860px) .side: static, 12px 16px, 8px gap, 61px resulting height.
  expect.soft(metrics.navigation).toEqual({ x: 0, y: 0, w: 390, h: 61 });
  expect.soft(metrics.navigationStyle).toEqual({ padding: '12px 16px', lineHeight: '20px', background: side, border: palette.hairline, position: 'static' });

  // Prototype .brand: 32px row, no mobile inset/margin, 10px gap; mark() is 22x22.
  expect.soft(metrics.brand).toEqual({ x: 16, y: 14, w: 70.36, h: 32 });
  expect.soft(metrics.brandStyle).toEqual({ padding: '0px', margin: '0px', gap: '10px', lineHeight: '20px' });
  expect.soft(metrics.mark).toEqual({ x: 16, y: 19, w: 22, h: 22 });

  // Prototype .brand b: 17px/20px weight 600, beginning at x48 after the 22px mark and 10px gap.
  expect.soft(metrics.wordmark).toEqual({ x: 48, y: 20, w: 38.36, h: 20 });
  expect.soft(metrics.wordmarkStyle).toEqual({ size: '17px', lineHeight: '20px', weight: '600', color: ink });

  // Prototype .mobile-pick and select.pick: fluid 32px field, 0 28px 0 12px, 13px with the native normal line height.
  expect.soft(metrics.selector).toEqual({ x: 94.36, y: 14, w: 235.64, h: 32 });
  expect.soft(metrics.selectorStyle).toEqual({ padding: '0px 28px 0px 12px', size: '13px', lineHeight: 'normal', color: ink, background: surface, border: palette.hairlineStrong, radius: '8px' });

  // Prototype mobile .side-foot button: 36x36, no border, 20px line, ink-2; icon is 16x16.
  expect.soft(metrics.theme).toEqual({ x: 338, y: 12, w: 36, h: 36 });
  expect.soft(metrics.themeStyle).toEqual({ padding: '0px', gap: '10px', borderWidth: '0px', lineHeight: '20px', whiteSpace: 'normal', textAlign: 'left', color: palette.secondary, background: 'rgba(0, 0, 0, 0)' });
  expect.soft(metrics.themeIcon).toEqual({ x: 348, y: 22, w: 16, h: 16 });
  expect.soft(metrics.themeIconColor).toBe(palette.secondary);

  // Prototype mobile .top: static, 8px 16px, 8px gap, page 88% + 8px blur, hairline.
  // Its own color stays --ink for the zero-width border paint; .crumb owns the muted text color.
  expect.soft(metrics.header).toMatchObject({ x: 0, y: 61, w: 390 });
  expect.soft(metrics.headerStyle).toMatchObject({ padding: '8px 16px', gap: '8px', size: '13px', lineHeight: '20px', color: ink, textAlign: 'start', position: 'static', border: palette.hairline, backdrop: 'blur(8px)' });
  const topMatch = /color\(srgb ([\d.]+) ([\d.]+) ([\d.]+) \/ ([\d.]+)\)/.exec(metrics.headerStyle?.background ?? '');
  expect.soft(topMatch, `mobile topbar background: ${metrics.headerStyle?.background}`).not.toBeNull();
  expect.soft(Number(topMatch![4])).toBeCloseTo(0.88, 2);
  for (const [index, channel] of palette.page.entries()) expect.soft(Number(topMatch![index + 1]) * 255).toBeCloseTo(channel, 0);

  // Prototype .crumb: the first text owner is muted 13px/20px; building icon 16px,
  // separator 14px, and the current page uses ink at weight 500.
  expect.soft(metrics.scopeStyle).toEqual({ size: '13px', lineHeight: '20px', color: palette.muted, textAlign: 'start' });
  expect.soft(metrics.buildingIcon).toMatchObject({ x: 16, y: 77, w: 16, h: 16 });
  expect.soft(metrics.currentCrumb).toMatchObject({ y: 75, h: 20 });
  expect.soft(metrics.currentCrumbStyle).toEqual({ size: '13px', lineHeight: '20px', weight: '500', color: ink });
  expect.soft(metrics.separator).toMatchObject({ y: 78, w: 14, h: 14 });
  // Prototype mobile .top > .av: display none (account data stays in the DOM for existing tests).
  expect.soft(metrics.avatarDisplay).toBe('none');

  // Prototype mobile .wrap: 0 16px 48px. The absolute y is not shared because the
  // prototype-only four-identity .as row and its 8px gap add 40px to the second bar.
  expect.soft(metrics.content).toMatchObject({ x: 0, w: 390 });
  expect.soft(metrics.contentStyle).toEqual({ padding: '0px 16px 48px', maxWidth: '1480px' });
  // Prototype mobile .head: 16px 0 12px; title remains 22px/28px.
  expect.soft(metrics.pageHeader).toMatchObject({ x: 16, w: 358, h: 56 });
  expect.soft(metrics.pageHeaderPadding).toBe('16px 0px 12px');
  expect.soft(metrics.title).toMatchObject({ x: 16, w: 110, h: 28 });
});

test('the shell changes at the same 860px breakpoint on both sides', async ({ page }) => {
  await page.setViewportSize({ width: 859, height: 844 });
  await page.goto(`${baseUrl}/activity?project=P-001&kind=development`);
  await page.locator('[data-testid="page-activity"]').waitFor();
  const read = () => page.evaluate(() => {
    const q = (selector: string) => document.querySelector(selector);
    const box = (el: Element | null) => {
      if (!el) return null;
      const r = el.getBoundingClientRect();
      return { x: r.left, y: r.top, w: r.width, h: r.height };
    };
    return {
      mobileNavigation: box(q('[data-testid="mobile-navigation"]')),
      selector: box(q('[data-testid="page-selector"]')),
      mobileTheme: box(q('[data-testid="mobile-theme-toggle"]')),
      sider: box(q('.shell-sider')),
      header: box(q('.shell-header')),
      contentPadding: q('.shell-content') ? getComputedStyle(q('.shell-content')!).padding : null,
      avatarDisplay: q('[data-testid="avatar-menu-trigger"]') ? getComputedStyle(q('[data-testid="avatar-menu-trigger"]')!).display : null,
    };
  });

  // Prototype @media(max-width:860px): at 859 the .side top bar and .mobile-pick replace the sider.
  const narrow = await read();
  expect.soft(narrow.mobileNavigation).toEqual({ x: 0, y: 0, w: 859, h: 61 });
  expect.soft(narrow.selector).toMatchObject({ y: 14, h: 32 });
  expect.soft(narrow.mobileTheme).toMatchObject({ w: 36, h: 36 });
  expect.soft(narrow.sider).toBeNull();
  expect.soft(narrow.header).toMatchObject({ x: 0, y: 61, w: 859 });
  expect.soft(narrow.contentPadding).toBe('0px 16px 48px');
  expect.soft(narrow.avatarDisplay).toBe('none');

  // The exact upper boundary remains compact too, pinning the CSS max-width rather than only the JS hook.
  await page.setViewportSize({ width: 860, height: 844 });
  const boundary = await read();
  expect.soft(boundary.mobileNavigation).toEqual({ x: 0, y: 0, w: 860, h: 61 });
  expect.soft(boundary.mobileTheme).toMatchObject({ w: 36, h: 36 });
  expect.soft(boundary.contentPadding).toBe('0px 16px 48px');
  expect.soft(boundary.sider).toBeNull();

  // At 861 the prototype rule no longer applies: 232px sticky sider, 56px desktop top,
  // no native page selector, visible avatar, and .wrap's 0 32px 64px content inset.
  await page.setViewportSize({ width: 861, height: 844 });
  await page.locator('.shell-sider').waitFor();
  const wide = await read();
  expect.soft(wide.mobileNavigation).toBeNull();
  expect.soft(wide.selector).toBeNull();
  expect.soft(wide.mobileTheme).toBeNull();
  expect.soft(wide.sider).toEqual({ x: 0, y: 0, w: 232, h: 844 });
  expect.soft(wide.header).toEqual({ x: 232, y: 0, w: 629, h: 56 });
  expect.soft(wide.contentPadding).toBe('0px 32px 64px');
  expect.soft(wide.avatarDisplay).toBe('flex');
});

test('the native mobile selector and theme button keep keyboard focus order', async ({ page }, testInfo) => {
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto(`${baseUrl}/activity?project=P-001&kind=development`);
  const selector = page.getByRole('combobox', { name: '页面' });
  const theme = page.getByTestId('mobile-theme-toggle');
  await selector.waitFor();
  await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
  await page.keyboard.press('Tab');
  await expect.soft(selector).toBeFocused();
  await page.keyboard.press('Tab');
  await expect.soft(theme).toBeFocused();
  await page.keyboard.press('Space');
  const expected = testInfo.project.name === 'desktop-dark' ? 'light' : 'dark';
  await expect.soft(page.locator('html')).toHaveAttribute('data-theme', expected);
  await expect.soft(theme).toBeFocused();
  // Selection and direct navigation stay covered by shell-responsive.test.tsx's native-select cases.
});
