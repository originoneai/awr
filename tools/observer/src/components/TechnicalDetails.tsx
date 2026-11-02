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
                const element = isValidElement(item.value);
                const raw = element ? null : rawObservationValue(item.value);
                const value = element ? item.value as ReactNode : raw ?? humanText('notProvided', i18n.language);
                // What the button copies: the raw value as the service gave it, or the copy text a value shown as an element declares.
                // A value that is missing has nothing to copy, so it offers no button (it used to copy the words "not provided").
                const copyText = element ? item.copyValue ?? null : raw;
                return (
                  <Space key={item.label} size={8} wrap data-testid={item.testId ?? `technical-detail-${item.label}`}>
                    <Typography.Text type="secondary" style={craftValueStyle}>{item.label}:</Typography.Text>
                    <Typography.Text code style={{ overflowWrap: 'anywhere' }}>{value}</Typography.Text>
                    {copyText == null ? null : (
                      <Button
                        type="text"
                        size="small"
                        icon={<Copy size={16} strokeWidth={1.5} aria-hidden />}
                        aria-label={`${humanText('copy', i18n.language)} ${item.label}`}
                        onClick={async () => {
                          try {
                            await navigator.clipboard.writeText(copyText);
                            setCopiedLabel(item.label);
                          } catch {
                            setCopiedLabel(null);
                          }
                        }}
                      >
                        {copiedLabel === item.label ? humanText('copied', i18n.language) : humanText('copy', i18n.language)}
                      </Button>
                    )}
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
