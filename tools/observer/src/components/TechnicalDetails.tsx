import { Button, Collapse, Space, Typography } from 'antd';
import { Copy } from 'lucide-react';
import { isValidElement, useState, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { humanText } from '../i18n/human-language';
import { rawObservationValue } from '../i18n/craft';
import { craftValueStyle } from '../theme';

export interface TechnicalDetailItem {
  label: string;
  value: unknown;
  testId?: string;
  copyValue?: string;
}

/** antd-managed disclosure: raw engineering values stay available on demand. */
export function TechnicalDetails({ items, children }: { items: TechnicalDetailItem[]; children?: ReactNode }) {
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
                const value = isValidElement(item.value)
                  ? item.value
                  : rawObservationValue(item.value) ?? humanText('notProvided', i18n.language);
                return (
                  <Space key={item.label} size={8} wrap data-testid={item.testId ?? `technical-detail-${item.label}`}>
                    <Typography.Text type="secondary" style={craftValueStyle}>{item.label}:</Typography.Text>
                    <Typography.Text code style={{ overflowWrap: 'anywhere' }}>{value}</Typography.Text>
                    <Button
                      type="text"
                      size="small"
                      icon={<Copy size={16} strokeWidth={1.5} aria-hidden />}
                      aria-label={`${humanText('copy', i18n.language)} ${item.label}`}
                      onClick={async () => {
                        try {
                          await navigator.clipboard.writeText(typeof value === 'string' ? value : item.copyValue ?? '');
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
              {children}
            </Space>
          ),
        }]}
      />
    </div>
  );
}
