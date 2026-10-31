import type { ThemeConfig } from 'antd';

export const themeConfig: ThemeConfig = {
  token: {
    colorPrimary: '#2563EB',
    borderRadius: 8,
  },
  components: {
    Layout: {
      headerHeight: 64,
      headerBg: '#ffffff',
      siderBg: '#ffffff',
    },
    Menu: {
      itemSelectedBg: '#EFF6FF',
      itemSelectedColor: '#2563EB',
    },
  },
};
