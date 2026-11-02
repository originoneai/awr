import { Tag } from 'antd';
import { useTranslation } from 'react-i18next';
import { claimState } from '../core/observation';

/** Whether somebody has taken a work up; a missing field reads "not provided", never "unclaimed". */
export function ClaimTag({ value }: { value: unknown }) {
  const { i18n } = useTranslation();
  const claim = claimState(value, { language: i18n.language });
  return <Tag color={claim.kind === 'claimed' ? 'processing' : 'default'}>{claim.text}</Tag>;
}
