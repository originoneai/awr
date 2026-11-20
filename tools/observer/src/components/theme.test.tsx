import { theme as antdTheme } from 'antd';
import { act, render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { afterEach, describe, expect, it, vi } from 'vitest';
import '../i18n';
import {
  applyCraftTheme,
  craftCssVariables,
  craftThemes,
  createThemeConfig,
  type CraftThemeMode,
} from '../theme';
import { CraftThemeProvider, THEME_STORAGE_KEY } from './CraftThemeProvider';
import { ThemeToggle } from './ThemeToggle';

function channel(hex: string, index: number): number {
  return Number.parseInt(hex.slice(index, index + 2), 16) / 255;
}

function luminance(hex: string): number {
  const [red, green, blue] = [1, 3, 5].map((index) => channel(hex, index)).map((value) => (
    value <= 0.04045 ? value / 12.92 : ((value + 0.055) / 1.055) ** 2.4
  ));
  return 0.2126 * red + 0.7152 * green + 0.0722 * blue;
}

function contrast(left: string, right: string): number {
  const [high, low] = [luminance(left), luminance(right)].sort((a, b) => b - a);
  return (high + 0.05) / (low + 0.05);
}

function ThemeProbe() {
  const { token } = antdTheme.useToken();
  return (
    <output
      data-testid="theme-probe"
      data-primary={token.colorPrimary}
      data-surface={token.colorBgContainer}
      data-on-primary={token.colorTextLightSolid}
    />
  );
}

function renderTheme() {
  return render(
    <CraftThemeProvider>
      <ThemeToggle />
      <ThemeProbe />
    </CraftThemeProvider>,
  );
}

function mockSystemTheme(initial: CraftThemeMode) {
  let dark = initial === 'dark';
  const listeners = new Set<() => void>();
  const media = {
    get matches() { return dark; },
    media: '(prefers-color-scheme: dark)',
    onchange: null,
    addListener: (listener: () => void) => listeners.add(listener),
    removeListener: (listener: () => void) => listeners.delete(listener),
    addEventListener: (_type: string, listener: () => void) => listeners.add(listener),
    removeEventListener: (_type: string, listener: () => void) => listeners.delete(listener),
    dispatchEvent: () => false,
  };
  window.matchMedia = vi.fn(() => media) as unknown as typeof window.matchMedia;
  return {
    setMode(mode: CraftThemeMode) {
      dark = mode === 'dark';
      for (const listener of listeners) listener();
    },
  };
}

const originalMatchMedia = window.matchMedia;

afterEach(() => {
  window.matchMedia = originalMatchMedia;
  window.localStorage.removeItem(THEME_STORAGE_KEY);
  delete document.documentElement.dataset.theme;
  document.documentElement.removeAttribute('style');
});

describe('complete prototype theme tokens', () => {
  it.each(['light', 'dark'] as const)('keeps every text role readable on every %s surface', (mode) => {
    const colors = craftThemes[mode].color;
    // muted is pinned separately: the owner restored the prototype weak ink, which is
    // deliberately below 4.5 on the page and raised surfaces (see the pin below).
    const roles = ['ink', 'secondary', 'accent', 'positive', 'negative', 'warning'] as const;
    const surfaces = ['page', 'side', 'surface', 'raised'] as const;
    for (const role of roles) {
      for (const surface of surfaces) {
        expect(contrast(colors[role], colors[surface]), `${mode} ${role} on ${surface}`)
          .toBeGreaterThanOrEqual(4.5);
      }
    }
    // The prototype weak ink stays readable on every side/surface tone in both themes.
    for (const surface of ['side', 'surface'] as const) {
      expect(contrast(colors.muted, colors[surface]), `${mode} muted on ${surface}`)
        .toBeGreaterThanOrEqual(4.5);
    }
  });

  it('restores the prototype weak ink exactly, including its below-4.5 pairs', () => {
    // Prototype --ink-3. The owner voided the earlier higher-contrast substitutes
    // (#626B7C / #7E869A) in favour of pixel parity with the prototype on 2026-11-19.
    expect(craftThemes.light.color.muted).toBe('#6B7487');
    expect(craftThemes.dark.color.muted).toBe('#7A8296');
    // The prototype's own contrast numbers, asserted tightly so drift either way fails.
    expect(contrast('#6B7487', craftThemes.light.color.page)).toBeCloseTo(4.30, 1);
    expect(contrast('#6B7487', craftThemes.light.color.raised)).toBeCloseTo(4.07, 1);
    expect(contrast('#7A8296', craftThemes.dark.color.raised)).toBeCloseTo(4.33, 1);
  });

  it('pins every neutral, accent, semantic, soft-base and line token in both themes', () => {
    expect(craftThemes.light.color).toEqual({
      page: '#F3F5F9', side: '#FFFFFF', surface: '#FFFFFF', raised: '#ECEFF4',
      ink: '#111827', secondary: '#4B5565', muted: '#6B7487',
      hairline: 'rgba(17, 24, 39, 0.08)', hairlineStrong: 'rgba(17, 24, 39, 0.16)',
      accent: '#1D4ED8', onAccent: '#FFFFFF', accentSoft: 'rgba(29, 78, 216, 0.08)',
      canvas: '#EEF1F6',
      positive: '#147B3B', positiveSoft: 'rgba(20, 123, 59, 0.10)',
      negative: '#B91C1C', negativeSoft: 'rgba(185, 28, 28, 0.08)',
      warning: '#925F0B', warningSoft: 'rgba(146, 95, 11, 0.10)',
    });
    expect(craftThemes.dark.color).toEqual({
      page: '#0B0D14', side: '#07080D', surface: '#11141C', raised: '#1A1E28',
      ink: '#EEF0F5', secondary: '#A3A9B8', muted: '#7A8296',
      hairline: 'rgba(238, 240, 245, 0.08)', hairlineStrong: 'rgba(238, 240, 245, 0.16)',
      accent: '#8AB0FF', onAccent: '#07080D', accentSoft: 'rgba(138, 176, 255, 0.12)',
      canvas: '#07080D',
      positive: '#4CC38A', positiveSoft: 'rgba(76, 195, 138, 0.14)',
      negative: '#F2686C', negativeSoft: 'rgba(242, 104, 108, 0.14)',
      warning: '#E0A63A', warningSoft: 'rgba(224, 166, 58, 0.14)',
    });
    expect(craftThemes.light.shadow).toEqual({
      small: '0 1px 2px rgba(17, 24, 39, 0.05)', floating: '0 8px 24px rgba(17, 24, 39, 0.10)',
    });
  });

  it('pins the shell geometry the prototype fixes (side 232, top 56, wrap 1480, brand 20px line)', () => {
    expect(craftThemes.light.siderWidth).toBe(232);
    expect(craftThemes.dark.siderWidth).toBe(232);
    expect(craftThemes.light.headerHeight).toBe(56);
    expect(craftThemes.light.contentMaxWidth).toBe(1480);
    expect(craftThemes.light.brand.wordmarkLineHeight).toBe(20);
    expect(craftCssVariables('light')['--craft-brand-wordmark-line-height']).toBe('20px');
  });

  it('maps both themes to the exact supplied categorical series', () => {
    expect(craftThemes.light.chart.series).toEqual(['#2A78D6', '#EB6834', '#1BAF7A', '#EDA100']);
    expect(craftThemes.dark.chart.series).toEqual(['#3987E5', '#D95926', '#199E70', '#C98500']);
  });

  it('provides four validated ordered-blue completion steps in both themes', () => {
    expect(craftThemes.light.chart.completion).toEqual({
      verified: '#104281', reported: '#256ABF', inProgress: '#5598E7', notStarted: '#86B6EF',
    });
    expect(craftThemes.dark.chart.completion).toEqual({
      verified: '#86B6EF', reported: '#3987E5', inProgress: '#256ABF', notStarted: '#184F95',
    });
  });

  it('shares the radius and motion ladder while replacing both shadows and every color', () => {
    expect(craftThemes.dark.radius).toEqual(craftThemes.light.radius);
    expect(craftThemes.dark.motion).toEqual(craftThemes.light.motion);
    expect(craftThemes.dark.shadow).toEqual({
      small: 'none', floating: '0 16px 40px rgba(0, 0, 0, 0.50)',
    });
    for (const role of Object.keys(craftThemes.light.color)) {
      expect(craftThemes.dark.color).toHaveProperty(role);
    }
  });

  it('names the prototype typography and measure tokens identically in both themes', () => {
    for (const mode of ['light', 'dark'] as const) {
      // metric/metricCompact join the set as the approved round-2 tokens (R7).
      expect(craftThemes[mode].text).toEqual({ small: 13, title: 22, metric: 32, metricCompact: 28 });
      expect(craftThemes[mode].leading).toEqual({ tight: 16, body: 20, title: 28, hero: 32, metric: 40, metricCompact: 36 });
      expect(craftThemes[mode].measure).toEqual({ summary: 760 });
      const variables = craftCssVariables(mode);
      expect(variables['--craft-text-small']).toBe('13px');
      expect(variables['--craft-text-title']).toBe('22px');
      expect(variables['--craft-leading-tight']).toBe('16px');
      expect(variables['--craft-leading-body']).toBe('20px');
      expect(variables['--craft-leading-title']).toBe('28px');
      expect(variables['--craft-leading-hero']).toBe('32px');
      expect(variables['--craft-measure-summary']).toBe('760px');
    }
  });

  it('names every prototype perspective measurement instead of borrowing unrelated antd tokens', () => {
    expect(craftThemes.light.perspective).toEqual({
      captionMaxWidth: 1040,
      gapToCaption: 8,
      gap: 2,
      padding: 2,
      optionHeight: 28,
      optionPaddingInline: 10,
      containerRadius: 8,
      optionRadius: 6,
      captionFontSize: 12,
      optionFontSize: 13,
      selectedLineWidth: 1,
      fontWeight: 400,
      selectedFontWeight: 500,
    });
    expect(craftThemes.dark.perspective).toEqual(craftThemes.light.perspective);
    expect(craftCssVariables('light')['--craft-perspective-radius']).toBe('6px');
    expect(craftCssVariables('light')['--craft-perspective-height']).toBe('28px');
    expect(craftCssVariables('light')['--craft-perspective-selected-shadow'])
      .toBe(`0 0 0 1px ${craftThemes.light.color.hairline}, ${craftThemes.light.shadow.small}`);
    expect(craftCssVariables('dark')['--craft-perspective-selected-shadow'])
      .toBe(`0 0 0 1px ${craftThemes.dark.color.hairline}`);
  });

  it.each(['light', 'dark'] as const)('uses the antd %s algorithm with token overrides', (mode) => {
    const config = createThemeConfig(mode);
    expect(config.algorithm).toEqual(expect.any(Function));
    expect(config.cssVar).toEqual({ key: 'awr-observer' });
    expect(config.token?.colorPrimary).toBe(craftThemes[mode].color.accent);
    expect(config.token?.colorTextLightSolid).toBe(craftThemes[mode].color.onAccent);
    expect(config.token?.colorSuccessBg).toBe(craftThemes[mode].color.positiveSoft);
    expect(config.token?.colorErrorBg).toBe(craftThemes[mode].color.negativeSoft);
    expect(config.token?.colorWarningBg).toBe(craftThemes[mode].color.warningSoft);
    expect(config.token?.colorInfoBg).toBe(craftThemes[mode].color.accentSoft);
    expect(config.token?.colorTextTertiary).toBe(craftThemes[mode].color.muted);
    expect(config.token?.colorBorderSecondary).toBe(craftThemes[mode].color.hairline);
    expect(config.token?.boxShadowSecondary).toBe(craftThemes[mode].shadow.floating);
    expect(config.token?.colorBorder).toBe(craftThemes[mode].color.hairlineStrong);
    expect(config.components?.Layout?.siderBg).toBe(craftThemes[mode].color.side);
  });

  it.each(['light', 'dark'] as const)('publishes a complete, defined CSS variable set for %s', (mode) => {
    const variables = craftCssVariables(mode);
    expect(Object.keys(variables).length).toBeGreaterThanOrEqual(35);
    for (const value of Object.values(variables)) {
      expect(value).not.toMatch(/undefined|null|NaN/);
      expect(value).not.toBe('');
    }
    expect(variables['--craft-completion-verified']).toBe(craftThemes[mode].chart.completion.verified);
    expect(variables['--craft-shadow-floating']).toBe(craftThemes[mode].shadow.floating);
  });

  it('updates the document color scheme and all custom properties synchronously', () => {
    applyCraftTheme(document.documentElement, 'dark');
    expect(document.documentElement.dataset.theme).toBe('dark');
    expect(document.documentElement.style.colorScheme).toBe('dark');
    expect(document.documentElement.style.getPropertyValue('--craft-color-surface'))
      .toBe(craftThemes.dark.color.surface);
  });
});

describe('theme selection behavior', () => {
  it('defaults to the system preference and follows system changes until explicitly chosen', () => {
    const media = mockSystemTheme('light');
    renderTheme();
    expect(document.documentElement.dataset.theme).toBe('light');
    expect(screen.getByRole('button', { name: '深色' })).toBeTruthy();
    expect(screen.getByTestId('theme-probe').getAttribute('data-primary')).toBe(craftThemes.light.color.accent);

    act(() => media.setMode('dark'));
    expect(document.documentElement.dataset.theme).toBe('dark');
    expect(screen.getByRole('button', { name: '浅色' })).toBeTruthy();
    expect(screen.getByTestId('theme-probe').getAttribute('data-primary')).toBe(craftThemes.dark.color.accent);
    expect(window.localStorage.getItem(THEME_STORAGE_KEY)).toBeNull();
  });

  it('loads a remembered choice over the system preference and persists the next choice', async () => {
    mockSystemTheme('light');
    window.localStorage.setItem(THEME_STORAGE_KEY, 'dark');
    const first = renderTheme();
    expect(document.documentElement.dataset.theme).toBe('dark');
    expect(screen.getByTestId('theme-probe').getAttribute('data-on-primary')).toBe(craftThemes.dark.color.onAccent);

    await userEvent.click(screen.getByRole('button', { name: '浅色' }));
    expect(window.localStorage.getItem(THEME_STORAGE_KEY)).toBe('light');
    expect(document.documentElement.dataset.theme).toBe('light');
    first.unmount();

    renderTheme();
    expect(document.documentElement.dataset.theme).toBe('light');
    expect(screen.getByRole('button', { name: '深色' })).toBeTruthy();
  });

  it('still toggles when local storage reads and writes are unavailable', async () => {
    mockSystemTheme('light');
    vi.spyOn(Storage.prototype, 'getItem').mockImplementation(() => { throw new Error('Blocked'); });
    vi.spyOn(Storage.prototype, 'setItem').mockImplementation(() => { throw new Error('Blocked'); });
    renderTheme();
    expect(document.documentElement.dataset.theme).toBe('light');

    await userEvent.click(screen.getByRole('button', { name: '深色' }));
    expect(document.documentElement.dataset.theme).toBe('dark');
    expect(screen.getByRole('button', { name: '浅色' })).toBeTruthy();
    expect(screen.getByTestId('theme-probe').getAttribute('data-surface')).toBe(craftThemes.dark.color.surface);
    expect(document.documentElement.style.getPropertyValue('--craft-color-surface'))
      .toBe(craftThemes.dark.color.surface);
  });
});
