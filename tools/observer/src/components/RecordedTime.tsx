import { Typography } from 'antd';
import { useTranslation } from 'react-i18next';
import { absoluteTimeText } from '../core/observation';
import { notProvided } from '../i18n/craft';
import { asText } from '../lib/json';
import { craftValueStyle } from '../theme';

/** A recorded time in the person's local clock. */
export function RecordedTime({ value }: { value: unknown }) {
  const { i18n } = useTranslation();
  const raw = asText(value);
  if (raw == null || raw.trim() === '' || !Number.isFinite(Number(raw))) return <>{notProvided(i18n.language)}</>;
  return (
    <Typography.Text className="craft-value" style={craftValueStyle}>
      {absoluteTimeText(Number(raw), { language: i18n.language }).text}
    </Typography.Text>
  );
}
