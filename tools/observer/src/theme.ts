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
  // Prototype .top height and .app grid column (232px) / .wrap max-width (1480px).
  headerHeight: 56,
  siderWidth: 232,
  // Prototype .brand and .brand b; the owner's supplied mark replaces the 22px prototype glyph.
  brand: {
    rowHeight: 32,
    markWidth: 24,
    markHeight: 18.75,
    gap: 10,
    paddingInline: 12,
    marginBottom: 20,
    wordmarkFontSize: 17,
    wordmarkFontWeight: 600,
    // Prototype .brand b inherits the body 20px line-height.
    wordmarkLineHeight: 20,
  },
  // Smallest touch target the mobile acceptance allows (drawer rows, header entry).
  touchTarget: 40,
  contentMaxWidth: 1480,
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
  },
  // Prototype activity(): .t column widths and .block-head h2 typography.
  activity: {
    columnTime: 168,
    columnActor: 232,
    columnTask: '24%',
    sectionTitleText: 14,
  },
  font: {
    interface: '-apple-system, BlinkMacSystemFont, "Segoe UI", "PingFang SC", "Hiragino Sans GB", "Noto Sans SC", "Microsoft YaHei", sans-serif',
    // Prototype --mono, verbatim (including Menlo).
    meta: 'ui-monospace, "SFMono-Regular", "Cascadia Mono", "Roboto Mono", Menlo, monospace',
  },
  // Prototype shell parts antd has no token for; global.css consumes them as variables.
  shell: {
    // Prototype .top: the page tone mixed at 88% and blurred 8px behind the bar.
    topbar: { pageMixPercent: 88, blurPx: 8 },
    // Prototype .av: a 28px circle with a 12px/500 initial.
    avatar: { size: 28, initialFontSize: 12, initialFontWeight: 500 },
  },
  // Prototype @media(max-width:860px): mark() and .side-foot button / .wrap.
  // Kept separate from the pinned desktop shell contract.
  mobileShell: { brandMarkSize: 22, themeButtonSize: 36, contentBottom: 48 },
  radius: {
    control: 8,
    card: 12,
    panel: 16,
    nested: 4,
    tag: 6,
  },
  // Named geometry for the metric strip, status tags and chain (review round 2, R7).
  height: {
    tag: 22,
    chainNode: 104,
  },
  gap: {
    tag: 6,
  },
  width: {
    lampRing: 1.5,
  },
  canvas: {
    // The chain's dotted canvas grid square.
    grid: 16,
  },
  // Named typography and measure tokens for prototype values antd has no token for.
  // theme.test pins the exact values; display.css and components consume them as CSS variables.
  text: {
    small: 13,
    title: 22,
    metric: 32,
    metricCompact: 28,
  },
  leading: {
    tight: 16,
    body: 20,
    title: 28,
    hero: 32,
    metric: 40,
    metricCompact: 36,
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
  // Workbench measurements transcribed from .inst, .word, .say, .inst-body and .btn.
  workbench: {
    railWidth: 348,
    healthHeight: 24,
    healthLamp: 10,
    healthPaddingInline: 10,
    captionText: 12,
    controlHeight: 32,
    verdictText: 24,
    verdictTextCompact: 20,
    verdictLeading: 32,
    verdictLeadingCompact: 28,
    agentLine: 160,
    agentColumn: 168,
    statusColumn: 108,
    ageColumn: 96,
    waitKindColumn: 136,
    waitDurationColumn: 150,
    waitBarMinWidth: 48,
    behindColumn: 104,
    // Prototype .cn { min-width: 0 } only survives on its four-node demo rows; real rows with
    // more nodes squeeze to a character. The row scrolls horizontally at this floor instead.
    // The floor must stay below the smallest natural share of a fitting row (the four-node
    // row hands each node ≈137px), so fitting rows keep the prototype's flex distribution.
    chainNodeMinWidth: 132,
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
      // Prototype --ink-3 (light). The owner voided the earlier higher-contrast substitute.
      muted: '#6B7487',
      hairline: 'rgba(17, 24, 39, 0.08)',
      hairlineStrong: 'rgba(17, 24, 39, 0.16)',
      accent: '#1D4ED8',
      onAccent: '#FFFFFF',
      accentSoft: 'rgba(29, 78, 216, 0.08)',
      // The chain block's dotted canvas (prototype --canvas, review round 2 R7).
      canvas: '#EEF1F6',
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
      // Prototype --ink-3 (dark).
      muted: '#7A8296',
      hairline: 'rgba(238, 240, 245, 0.08)',
      hairlineStrong: 'rgba(238, 240, 245, 0.16)',
      accent: '#8AB0FF',
      onAccent: '#07080D',
      accentSoft: 'rgba(138, 176, 255, 0.12)',
      // The chain block's dotted canvas (prototype --canvas, review round 2 R7).
      canvas: '#07080D',
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
  const {
    color, chart, shadow, radius, motion, font, text, leading, measure,
    height, gap, width, canvas, perspective, workbench, activity, brand, shell, mobileShell,
  } = tokens;
  return {
    '--craft-workbench-rail-width': `${workbench.railWidth}px`,
    '--craft-workbench-health-height': `${workbench.healthHeight}px`,
    '--craft-workbench-health-lamp': `${workbench.healthLamp}px`,
    '--craft-workbench-health-padding-inline': `${workbench.healthPaddingInline}px`,
    '--craft-workbench-caption-text': `${workbench.captionText}px`,
    '--craft-workbench-control-height': `${workbench.controlHeight}px`,
    '--craft-workbench-verdict-text': `${workbench.verdictText}px`,
    '--craft-workbench-verdict-text-compact': `${workbench.verdictTextCompact}px`,
    '--craft-workbench-verdict-leading': `${workbench.verdictLeading}px`,
    '--craft-workbench-verdict-leading-compact': `${workbench.verdictLeadingCompact}px`,
    '--craft-measure-agent-line': `${workbench.agentLine}px`,
    '--craft-workbench-column-agent': `${workbench.agentColumn}px`,
    '--craft-workbench-column-status': `${workbench.statusColumn}px`,
    '--craft-workbench-column-age': `${workbench.ageColumn}px`,
    '--craft-workbench-column-wait-kind': `${workbench.waitKindColumn}px`,
    '--craft-workbench-column-wait-duration': `${workbench.waitDurationColumn}px`,
    '--craft-workbench-wait-bar-min': `${workbench.waitBarMinWidth}px`,
    '--craft-workbench-column-behind': `${workbench.behindColumn}px`,
    '--craft-workbench-chain-node-min': `${workbench.chainNodeMinWidth}px`,
    '--craft-perspective-option-padding-inline': `${perspective.optionPaddingInline}px`,
    '--craft-activity-column-time': `${activity.columnTime}px`,
    '--craft-activity-column-actor': `${activity.columnActor}px`,
    '--craft-activity-column-task': activity.columnTask,
    '--craft-activity-section-title-text': `${activity.sectionTitleText}px`,
    '--craft-shell-topbar-page-mix': `${shell.topbar.pageMixPercent}%`,
    '--craft-shell-topbar-blur': `${shell.topbar.blurPx}px`,
    '--craft-shell-avatar-size': `${shell.avatar.size}px`,
    '--craft-shell-avatar-initial-font-size': `${shell.avatar.initialFontSize}px`,
    '--craft-shell-avatar-initial-font-weight': String(shell.avatar.initialFontWeight),
    '--craft-shell-mobile-brand-mark-size': `${mobileShell.brandMarkSize}px`,
    '--craft-shell-mobile-theme-button-size': `${mobileShell.themeButtonSize}px`,
    '--craft-shell-mobile-content-bottom': `${mobileShell.contentBottom}px`,
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
    '--craft-color-canvas': color.canvas,
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
    '--craft-radius-tag': `${radius.tag}px`,
    '--craft-brand-row-height': `${brand.rowHeight}px`,
    '--craft-brand-mark-width': `${brand.markWidth}px`,
    '--craft-brand-mark-height': `${brand.markHeight}px`,
    '--craft-brand-gap': `${brand.gap}px`,
    '--craft-brand-padding-inline': `${brand.paddingInline}px`,
    '--craft-brand-margin-bottom': `${brand.marginBottom}px`,
    '--craft-brand-wordmark-font-size': `${brand.wordmarkFontSize}px`,
    '--craft-brand-wordmark-font-weight': String(brand.wordmarkFontWeight),
    '--craft-brand-wordmark-line-height': `${brand.wordmarkLineHeight}px`,
    '--craft-height-tag': `${height.tag}px`,
    '--craft-height-chain-node': `${height.chainNode}px`,
    '--craft-gap-tag': `${gap.tag}px`,
    '--craft-width-lamp-ring': `${width.lampRing}px`,
    // The dotted canvas image lives here because the craft guard keeps gradient functions
    // out of component styles; theme.ts is the one exempt definition point.
    '--craft-canvas-grid-image': `radial-gradient(${color.hairlineStrong} 1px, transparent 1px)`,
    // The prototype inbox page's weak text (panorama --ink-3). The approved global muted
    // role keeps its higher contrast; this page-local pair stays a prototype value
    // (token-gap list in the AWR-PPO-030 PR).
    '--craft-inbox-muted': mode === 'dark' ? '#7A8296' : '#6B7487',
    // The prototype's mono stack keeps Menlo; the shared meta font stops one family earlier.
    '--craft-inbox-mono': 'ui-monospace, "SFMono-Regular", "Cascadia Mono", "Roboto Mono", Menlo, monospace',
    '--craft-size-canvas-grid': `${canvas.grid}px`,
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
    '--craft-text-metric': `${text.metric}px`,
    '--craft-text-metric-compact': `${text.metricCompact}px`,
    '--craft-leading-tight': `${leading.tight}px`,
    '--craft-leading-body': `${leading.body}px`,
    '--craft-leading-title': `${leading.title}px`,
    '--craft-leading-hero': `${leading.hero}px`,
    '--craft-leading-metric': `${leading.metric}px`,
    '--craft-leading-metric-compact': `${leading.metricCompact}px`,
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
