import { Button, Collapse, Space, Typography } from 'antd';
import { Copy } from 'lucide-react';
import { useState } from 'react';
import { useTranslation } from 'react-i18next';
import { humanText } from '../i18n/human-language';
import { rawObservationValue } from '../i18n/craft';
import { craftValueStyle } from '../theme';

export interface TechnicalDetailItem {
  label: string;
  value: unknown;
}

/** antd-managed disclosure: raw engineering values stay available on demand. */
export function TechnicalDetails({ items }: { items: TechnicalDetailItem[] }) {
  const { i18n } = useTranslation();
  const [copiedLabel, setCopiedLabel] = useState<string | null>(null);
  if (items.length === 0) return null;
  return (
    <div data-testid="technical-details">
      <Collapse
        ghost
        destroyOnHidden
        items={[{
          key: 'details',
          label: humanText('technicalDetails', i18n.language),
          children: (
            <Space direction="vertical" size={8} style={{ width: '100%' }}>
              {items.map((item) => {
                const value = rawObservationValue(item.value) ?? humanText('notProvided', i18n.language);
                return (
                  <Space key={item.label} size={8} wrap data-testid={`technical-detail-${item.label}`}>
                    <Typography.Text type="secondary" style={craftValueStyle}>{item.label}:</Typography.Text>
                    <Typography.Text code style={{ overflowWrap: 'anywhere' }}>{value}</Typography.Text>
                    <Button
                      type="text"
                      size="small"
                      icon={<Copy size={16} strokeWidth={1.5} aria-hidden />}
                      aria-label={`${humanText('copy', i18n.language)} ${item.label}`}
                      onClick={async () => {
                        try {
                          await navigator.clipboard.writeText(value);
                          setCopiedLabel(item.label);
                        } catch {
                          setCopiedLabel(null);
                        }
                      }}
                    >
                      {copiedLabel === item.label ? humanText('copied', i18n.language) : humanText('copy', i18n.language)}
                    </Button>
                  </Space>
                );
              })}
            </Space>
          ),
        }]}
      />
    </div>
  );
}
