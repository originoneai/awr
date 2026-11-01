import { Tooltip, Typography } from 'antd';
import { useTranslation } from 'react-i18next';
import { fieldLabel, formatObservationValue, rawObservationValue } from '../i18n/craft';
import { craftValueStyle } from '../theme';

export function FieldLabel({ field }: { field: string }) {
  const { i18n } = useTranslation();
  return (
    <Tooltip title={field}>
      <span>{fieldLabel(field, i18n.language)}</span>
    </Tooltip>
  );
}

/** Localized display with the exact service value always available in the tooltip. */
export function ObservationValue({ value, testId, field }: { value: unknown; testId?: string; field?: string }) {
  const { i18n } = useTranslation();
  const raw = rawObservationValue(value);
  const text = (
    <Typography.Text className="craft-value" data-testid={testId} style={{ ...craftValueStyle, overflowWrap: 'anywhere' }}>
      {formatObservationValue(value, i18n.language, field)}
    </Typography.Text>
  );
  return raw == null ? text : <Tooltip title={raw}>{text}</Tooltip>;
}
