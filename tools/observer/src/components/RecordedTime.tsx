import { Tooltip, Typography } from 'antd';
import { useTranslation } from 'react-i18next';
import { absoluteTimeText } from '../core/observation';
import { notProvided } from '../i18n/craft';
import { asText } from '../lib/json';
import { craftValueStyle } from '../theme';

/** A recorded time as the person's own clock reads it, the exact value on hover; not a number means not provided. */
export function RecordedTime({ value }: { value: unknown }) {
  const { i18n } = useTranslation();
  const raw = asText(value);
  if (raw == null || raw.trim() === '' || !Number.isFinite(Number(raw))) return <>{notProvided(i18n.language)}</>;
  return (
    <Tooltip title={raw}>
      <Typography.Text className="craft-value" style={craftValueStyle}>
        {absoluteTimeText(Number(raw), { language: i18n.language }).text}
      </Typography.Text>
    </Tooltip>
  );
}
