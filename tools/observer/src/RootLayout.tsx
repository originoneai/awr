import { App as AntdApp, ConfigProvider } from 'antd';
import enUS from 'antd/locale/en_US';
import zhCN from 'antd/locale/zh_CN';
import { useEffect, useState } from 'react';
import { useTranslation } from 'react-i18next';
import { Outlet, ScrollRestoration } from 'react-router';
import { AppShell } from './components/AppShell';
import { ScopeProvider } from './lib/scope';
import { themeConfig } from './theme';

function antdLocale(language: string) {
  return language === 'zh-CN' ? zhCN : enUS;
}

export function RootLayout() {
  const { i18n } = useTranslation();
  const [locale, setLocale] = useState(() => antdLocale(i18n.language));

  useEffect(() => {
    const onChanged = (language: string) => setLocale(antdLocale(language));
    i18n.on('languageChanged', onChanged);
    return () => {
      i18n.off('languageChanged', onChanged);
    };
  }, [i18n]);

  return (
    <ConfigProvider
      locale={locale}
      theme={themeConfig}
      csp={{ nonce: typeof window !== 'undefined' ? window.__CSP_NONCE__ : undefined }}
    >
      <AntdApp>
        <ScopeProvider>
          <AppShell>
            <Outlet />
          </AppShell>
          <ScrollRestoration />
        </ScopeProvider>
      </AntdApp>
    </ConfigProvider>
  );
}
