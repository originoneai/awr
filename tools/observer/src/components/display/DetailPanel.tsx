import { Tabs } from 'antd';
import type { ReactNode } from 'react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { useCompactViewport } from '../../lib/viewport';
import './display.css';

export interface DetailPanelTab {
  key: string;
  label: ReactNode;
  count?: number;
  content: ReactNode;
  footer?: ReactNode;
}

export interface DetailPanelProps {
  tabs: readonly DetailPanelTab[];
  activeKey?: string;
  defaultActiveKey?: string;
  onChange?: (key: string) => void;
  extra?: ReactNode;
}

/** The panel owns tab composition; each tab owns only its content and optional footer. */
export function DetailPanel({ tabs, activeKey, defaultActiveKey, onChange, extra }: DetailPanelProps) {
  const { t } = useTranslation();
  const compactViewport = useCompactViewport();
  const [internalKey, setInternalKey] = useState(defaultActiveKey ?? tabs[0]?.key);
  const selectedKey = activeKey ?? internalKey;
  const activeTab = tabs.find((tab) => tab.key === selectedKey) ?? tabs[0];
  const resolvedKey = activeTab?.key;

  const selectTab = (key: string) => {
    if (activeKey === undefined) setInternalKey(key);
    onChange?.(key);
  };

  return (
    <section className="obs-detail-panel" aria-label={t('display.detailPanel.label')} data-compact={compactViewport || undefined}>
      <Tabs
        className="obs-detail-tabs"
        activeKey={resolvedKey}
        items={tabs.map((tab) => ({
          key: tab.key,
          label: (
            <span className="obs-detail-tab-label">
              {tab.label}
              {tab.count === undefined ? null : <span className="obs-detail-tab-count">{tab.count}</span>}
            </span>
          ),
          children: <div className="obs-detail-tab-content" data-tab-key={tab.key}>{tab.content}</div>,
        }))}
        animated={false}
        destroyOnHidden
        onChange={selectTab}
        tabBarExtraContent={{ right: extra }}
      />
      {activeTab?.footer === undefined ? null : <div className="obs-detail-footer">{activeTab.footer}</div>}
    </section>
  );
}
