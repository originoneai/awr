import type { ThemeConfig } from 'antd';

/** Project craft tokens. Visual values are centralized here, not in pages. */
export const craftTokens = {
  spacing: {
    page: 24,
    section: 32,
    group: 16,
    item: 8,
    compact: 4,
  },
  navigationRowHeight: 36,
  headerHeight: 64,
  siderWidth: 240,
  contentMaxWidth: 1600,
} as const;

export const themeConfig: ThemeConfig = {
  token: {
    colorPrimary: '#2563EB',
    colorBgLayout: '#F8FAFC',
    colorSplit: '#E2E8F0',
    borderRadius: 8,
    controlHeight: craftTokens.navigationRowHeight,
    padding: craftTokens.spacing.page,
    paddingSM: craftTokens.spacing.group,
    paddingXS: craftTokens.spacing.item,
    margin: craftTokens.spacing.group,
    marginSM: craftTokens.spacing.item,
    marginXS: craftTokens.spacing.compact,
  },
  components: {
    Layout: {
      headerHeight: craftTokens.headerHeight,
      headerBg: '#FFFFFF',
      siderBg: '#FFFFFF',
      bodyBg: '#F8FAFC',
    },
    Menu: {
      itemHeight: craftTokens.navigationRowHeight,
      itemColor: '#334155',
      itemHoverBg: '#F8FAFC',
      itemSelectedBg: '#EFF6FF',
      itemSelectedColor: '#2563EB',
      itemMarginInline: 12,
      itemMarginBlock: craftTokens.spacing.compact,
      iconSize: 18,
    },
  },
};
