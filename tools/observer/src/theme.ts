import type { ThemeConfig } from 'antd';

export const themeConfig: ThemeConfig = {
  token: {
    colorPrimary: '#2563EB',
    colorBgLayout: '#F8FAFC',
    colorSplit: '#E2E8F0',
    borderRadius: 8,
  },
  components: {
    Layout: {
      headerHeight: 64,
      headerBg: '#FFFFFF',
      siderBg: '#FFFFFF',
      bodyBg: '#F8FAFC',
    },
    Menu: {
      itemHeight: 52,
      itemColor: '#334155',
      itemHoverBg: '#F8FAFC',
      itemSelectedBg: '#EFF6FF',
      itemSelectedColor: '#2563EB',
      itemMarginInline: 12,
      itemMarginBlock: 2,
      iconSize: 18,
    },
  },
};
