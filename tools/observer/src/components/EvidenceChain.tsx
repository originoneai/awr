import { Steps, Typography } from 'antd';
import { useTranslation } from 'react-i18next';
import { ObservationValue } from './ObservationValue';

function stepStatus(value: unknown, current: boolean): 'finish' | 'process' | 'wait' | 'error' {
  if (value === 'rejected' || value === 'invalidated' || value === 'failed') return 'error';
  if (value === 'approved' || value === 'confirmed' || value === 'completed' || value === 'succeeded' || value === true) return 'finish';
  if (current || value === 'open' || value === 'dispatched' || value === 'active') return 'process';
  return 'wait';
}

/** A presentation of existing evidence facts; it never infers a new delivery state. */
export function EvidenceChain({
  candidateCurrent,
  reviewState,
  reviewCurrent,
  integrationState,
  completionState,
  completionReceipt,
}: {
  candidateCurrent?: boolean;
  reviewState?: unknown;
  reviewCurrent: boolean;
  integrationState?: unknown;
  completionState?: unknown;
  completionReceipt?: unknown;
}) {
  const { t } = useTranslation();
  const items = [
    {
      title: t('acceptance.chainCandidate'),
      value: candidateCurrent,
      current: candidateCurrent === true,
    },
    {
      title: t('acceptance.chainReview'),
      value: reviewState,
      current: reviewCurrent,
    },
    {
      title: t('acceptance.chainIntegration'),
      value: integrationState,
      current: integrationState === 'dispatched',
    },
    {
      title: t('acceptance.chainCompletion'),
      value: completionReceipt == null || completionReceipt === '' ? null : completionState,
      current: false,
      description: completionReceipt == null || completionReceipt === ''
        ? <Typography.Text type="secondary">{t('acceptance.noCompletionReceipt')}</Typography.Text>
        : <ObservationValue value={completionReceipt} />,
    },
  ];
  return (
    <Steps
      direction="horizontal"
      size="small"
      items={items.map((item) => ({
        title: <Typography.Text>{item.title}</Typography.Text>,
        description: item.description ?? <ObservationValue value={item.value} field="state" />,
        status: stepStatus(item.value, item.current),
      }))}
    />
  );
}
