import { theme as antdTheme, type ThemeConfig } from 'antd';

const sharedCraftTokens = {
  spacing: {
    page: 24,
    section: 32,
    group: 16,
    item: 8,
    compact: 4,
  },
  navigationRowHeight: 36,
  controlHeight: 36,
  headerHeight: 64,
  siderWidth: 240,
  // Smallest touch target the mobile acceptance allows (drawer rows, header entry).
  touchTarget: 40,
  contentMaxWidth: 1600,
  // Prototype .as, .seg and .seg button; the caption has its own non-layout breakpoint.
  perspective: {
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
    limitationGap: 4,
  },
  font: {
    interface: '-apple-system, BlinkMacSystemFont, "Segoe UI", "PingFang SC", "Hiragino Sans GB", "Noto Sans SC", "Microsoft YaHei", sans-serif',
    meta: 'ui-monospace, "SFMono-Regular", "Cascadia Mono", "Roboto Mono", monospace',
  },
  radius: {
    control: 8,
    card: 12,
    panel: 16,
    nested: 4,
  },
  // Named typography and measure tokens for prototype values antd has no token for.
  // theme.test pins the exact values; display.css and components consume them as CSS variables.
  text: {
    small: 13,
    title: 22,
  },
  leading: {
    tight: 16,
    body: 20,
    title: 28,
    hero: 32,
  },
  measure: {
    summary: 760,
  },
  motion: {
    easing: 'cubic-bezier(0.22, 1, 0.36, 1)',
    feedbackMs: 160,
    overlayMs: 200,
    enterMs: 400,
  },
} as const;

/** Every visual role has a complete light and dark value; shared geometry uses the same scale. */
export const craftThemes = {
  light: {
    ...sharedCraftTokens,
    color: {
      page: '#F3F5F9',
      side: '#FFFFFF',
      surface: '#FFFFFF',
      raised: '#ECEFF4',
      ink: '#111827',
      secondary: '#4B5565',
      muted: '#626B7C',
      hairline: 'rgba(17, 24, 39, 0.08)',
      hairlineStrong: 'rgba(17, 24, 39, 0.16)',
      accent: '#1D4ED8',
      onAccent: '#FFFFFF',
      accentSoft: 'rgba(29, 78, 216, 0.08)',
      positive: '#147B3B',
      positiveSoft: 'rgba(20, 123, 59, 0.10)',
      negative: '#B91C1C',
      negativeSoft: 'rgba(185, 28, 28, 0.08)',
      warning: '#925F0B',
      warningSoft: 'rgba(146, 95, 11, 0.10)',
    },
    chart: {
      series: ['#2A78D6', '#EB6834', '#1BAF7A', '#EDA100'],
      completion: {
        verified: '#104281',
        reported: '#256ABF',
        inProgress: '#5598E7',
        notStarted: '#86B6EF',
      },
    },
    shadow: {
      small: '0 1px 2px rgba(17, 24, 39, 0.05)',
      floating: '0 8px 24px rgba(17, 24, 39, 0.10)',
    },
  },
  dark: {
    ...sharedCraftTokens,
    color: {
      page: '#0B0D14',
      side: '#07080D',
      surface: '#11141C',
      raised: '#1A1E28',
      ink: '#EEF0F5',
      secondary: '#A3A9B8',
      muted: '#7E869A',
      hairline: 'rgba(238, 240, 245, 0.08)',
      hairlineStrong: 'rgba(238, 240, 245, 0.16)',
      accent: '#8AB0FF',
      onAccent: '#07080D',
      accentSoft: 'rgba(138, 176, 255, 0.12)',
      positive: '#4CC38A',
      positiveSoft: 'rgba(76, 195, 138, 0.14)',
      negative: '#F2686C',
      negativeSoft: 'rgba(242, 104, 108, 0.14)',
      warning: '#E0A63A',
      warningSoft: 'rgba(224, 166, 58, 0.14)',
    },
    chart: {
      series: ['#3987E5', '#D95926', '#199E70', '#C98500'],
      completion: {
        verified: '#86B6EF',
        reported: '#3987E5',
        inProgress: '#256ABF',
        notStarted: '#184F95',
      },
    },
    shadow: {
      small: 'none',
      floating: '0 16px 40px rgba(0, 0, 0, 0.50)',
    },
  },
} as const;

export type CraftThemeMode = keyof typeof craftThemes;

/** Backwards-compatible light defaults; themed surfaces use craftThemes or the CSS variables below. */
export const craftTokens = craftThemes.light;

export const craftValueStyle = {
  color: 'var(--craft-color-secondary)',
  fontFamily: craftTokens.font.meta,
  fontVariantNumeric: 'tabular-nums',
  fontWeight: 400,
  letterSpacing: 0,
} as const;

export function craftSelectionStyle(
  token: { colorFillSecondary: string; colorBorderSecondary: string; borderRadius: number },
  selected: boolean,
) {
  if (!selected) return { cursor: 'pointer' } as const;
  return {
    cursor: 'pointer',
    background: token.colorFillSecondary,
    boxShadow: `inset 0 0 0 1px ${token.colorBorderSecondary}`,
    borderRadius: token.borderRadius,
  } as const;
}

function createBaseThemeConfig(mode: CraftThemeMode): ThemeConfig {
  const tokens = craftThemes[mode];
  const colors = tokens.color;
  return {
    algorithm: mode === 'dark' ? antdTheme.darkAlgorithm : antdTheme.defaultAlgorithm,
    token: {
      fontFamily: tokens.font.interface,
      fontFamilyCode: tokens.font.meta,
      fontWeightStrong: 500,
      motionEaseOut: tokens.motion.easing,
      motionDurationFast: `${tokens.motion.feedbackMs}ms`,
      motionDurationMid: `${tokens.motion.overlayMs}ms`,
      motionDurationSlow: `${tokens.motion.enterMs}ms`,
      fontSize: 14,
      fontSizeSM: 12,
      fontSizeHeading1: 32,
      fontSizeHeading2: 28,
      fontSizeHeading3: 24,
      fontSizeHeading4: 20,
      fontSizeHeading5: 16,
      colorPrimary: colors.accent,
      colorLink: colors.accent,
      colorText: colors.ink,
      colorTextSecondary: colors.secondary,
      colorTextTertiary: colors.muted,
      colorTextLightSolid: colors.onAccent,
      colorBgLayout: colors.page,
      colorBgContainer: colors.surface,
      colorBgElevated: colors.surface,
      colorFillSecondary: colors.raised,
      colorBorder: colors.hairlineStrong,
      colorBorderSecondary: colors.hairline,
      colorSplit: colors.hairline,
      colorSuccess: colors.positive,
      colorSuccessBg: colors.positiveSoft,
      colorSuccessBorder: colors.hairlineStrong,
      colorError: colors.negative,
      colorErrorBg: colors.negativeSoft,
      colorErrorBorder: colors.hairlineStrong,
      colorWarning: colors.warning,
      colorWarningBg: colors.warningSoft,
      colorWarningBorder: colors.hairlineStrong,
      colorInfo: colors.accent,
      colorInfoBg: colors.accentSoft,
      colorInfoBorder: colors.hairlineStrong,
      borderRadius: tokens.radius.control,
      borderRadiusLG: tokens.radius.card,
      borderRadiusSM: tokens.radius.nested,
      boxShadowTertiary: tokens.shadow.small,
      boxShadowSecondary: tokens.shadow.floating,
      controlHeight: tokens.controlHeight,
      padding: tokens.spacing.page,
      paddingSM: tokens.spacing.group,
      paddingXS: tokens.spacing.item,
      margin: tokens.spacing.group,
      marginSM: tokens.spacing.item,
      marginXS: tokens.spacing.compact,
    },
    components: {
      Layout: {
        headerHeight: tokens.headerHeight,
        headerBg: colors.surface,
        siderBg: colors.side,
        bodyBg: colors.page,
      },
      Card: {
        borderRadiusLG: tokens.radius.card,
      },
      Menu: {
        itemHeight: tokens.navigationRowHeight,
        itemBorderRadius: tokens.radius.control,
        itemColor: colors.secondary,
        itemHoverBg: colors.raised,
        itemSelectedBg: colors.accentSoft,
        itemSelectedColor: colors.accent,
        itemMarginInline: 0,
        itemMarginBlock: 0,
        itemPaddingInline: 12,
        iconMarginInlineEnd: 10,
        iconSize: 16,
      },
    },
  };
}

export function createThemeConfig(mode: CraftThemeMode): ThemeConfig {
  const config = createBaseThemeConfig(mode);
  const baseAlgorithm = mode === 'dark' ? antdTheme.darkAlgorithm : antdTheme.defaultAlgorithm;
  return {
    ...config,
    cssVar: { key: 'awr-observer' },
    // antd's dark algorithm derives a different primary from the seed. Reapply every project
    // override to the algorithm result so runtime tokens exactly match the prototype values.
    algorithm: (seed) => ({ ...baseAlgorithm(seed), ...config.token }),
  };
}

export const themeConfig = createThemeConfig('light');

/** CSS custom properties expose chart and completion roles that antd does not own. */
export function craftCssVariables(mode: CraftThemeMode): Record<string, string> {
  const tokens = craftThemes[mode];
  const { color, chart, shadow, radius, motion, font, text, leading, measure, perspective } = tokens;
  return {
    '--craft-color-page': color.page,
    '--craft-color-side': color.side,
    '--craft-color-surface': color.surface,
    '--craft-color-raised': color.raised,
    '--craft-color-ink': color.ink,
    '--craft-color-secondary': color.secondary,
    '--craft-color-muted': color.muted,
    '--craft-color-hairline': color.hairline,
    '--craft-color-hairline-strong': color.hairlineStrong,
    '--craft-color-accent': color.accent,
    '--craft-color-on-accent': color.onAccent,
    '--craft-color-accent-soft': color.accentSoft,
    '--craft-color-positive': color.positive,
    '--craft-color-positive-soft': color.positiveSoft,
    '--craft-color-negative': color.negative,
    '--craft-color-negative-soft': color.negativeSoft,
    '--craft-color-warning': color.warning,
    '--craft-color-warning-soft': color.warningSoft,
    '--craft-chart-series-1': chart.series[0],
    '--craft-chart-series-2': chart.series[1],
    '--craft-chart-series-3': chart.series[2],
    '--craft-chart-series-4': chart.series[3],
    '--craft-completion-verified': chart.completion.verified,
    '--craft-completion-reported': chart.completion.reported,
    '--craft-completion-in-progress': chart.completion.inProgress,
    '--craft-completion-not-started': chart.completion.notStarted,
    '--craft-shadow-small': shadow.small,
    '--craft-shadow-floating': shadow.floating,
    '--craft-radius-control': `${radius.control}px`,
    '--craft-radius-card': `${radius.card}px`,
    '--craft-radius-panel': `${radius.panel}px`,
    '--craft-radius-nested': `${radius.nested}px`,
    '--craft-perspective-radius': `${perspective.optionRadius}px`,
    '--craft-perspective-height': `${perspective.optionHeight}px`,
    '--craft-perspective-padding': `${perspective.padding}px`,
    '--craft-perspective-gap': `${perspective.gap}px`,
    // "none" cannot be a second box-shadow list item; dark keeps the hairline on its own.
    '--craft-perspective-selected-shadow': shadow.small === 'none'
      ? `0 0 0 ${perspective.selectedLineWidth}px ${color.hairline}`
      : `0 0 0 ${perspective.selectedLineWidth}px ${color.hairline}, ${shadow.small}`,
    '--craft-text-small': `${text.small}px`,
    '--craft-text-title': `${text.title}px`,
    '--craft-leading-tight': `${leading.tight}px`,
    '--craft-leading-body': `${leading.body}px`,
    '--craft-leading-title': `${leading.title}px`,
    '--craft-leading-hero': `${leading.hero}px`,
    '--craft-measure-summary': `${measure.summary}px`,
    '--craft-motion-easing': motion.easing,
    '--craft-motion-feedback': `${motion.feedbackMs}ms`,
    '--craft-motion-overlay': `${motion.overlayMs}ms`,
    '--craft-motion-enter': `${motion.enterMs}ms`,
    '--craft-font-interface': font.interface,
    '--craft-font-meta': font.meta,
  };
}

/** Apply root variables in a layout effect, before the browser paints a theme change. */
export function applyCraftTheme(root: HTMLElement, mode: CraftThemeMode) {
  root.dataset.theme = mode;
  root.style.colorScheme = mode;
  for (const [property, value] of Object.entries(craftCssVariables(mode))) {
    root.style.setProperty(property, value);
  }
}
