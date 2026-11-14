import { Typography } from 'antd';
import { useTranslation } from 'react-i18next';
import { fieldLabel, formatObservationValue } from '../i18n/craft';
import { craftValueStyle } from '../theme';

export function FieldLabel({ field }: { field: string }) {
  const { i18n } = useTranslation();
  return <span>{fieldLabel(field, i18n.language)}</span>;
}

/** Business labels stay readable in every interaction state. */
export function ObservationValue({ value, testId, field }: { value: unknown; testId?: string; field?: string }) {
  const { i18n } = useTranslation();
  return (
    <Typography.Text className="craft-value" data-testid={testId} style={{ ...craftValueStyle, overflowWrap: 'anywhere' }}>
      {formatObservationValue(value, i18n.language, field)}
    </Typography.Text>
  );
}
