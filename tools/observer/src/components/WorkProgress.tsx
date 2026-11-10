import { Space } from 'antd';
import { useTranslation } from 'react-i18next';
import { deriveWorkState, statusWord } from '../core/observation';
import { readStateOf } from '../lib/read-state';
import type { SnapshotQuery } from '../lib/work-snapshots';
import type { TeamWork } from '../lib/types';
import { ToneBadge } from './ToneBadge';

/** What the list row and the work's own read together say about one work. */
export function useWorkState(work: TeamWork, query?: SnapshotQuery) {
  const { i18n } = useTranslation();
  return deriveWorkState({
    language: i18n.language,
    readState: readStateOf(query),
    workNextStep: work.next_step,
    workStatus: work.status,
    snapshot: query?.data?.result.data,
  });
}

/** How far the work has come, as one word with the dot of its tone. */
export function WorkProgress({ work, query, testId }: { work: TeamWork; query?: SnapshotQuery; testId: string }) {
  const { t, i18n } = useTranslation();
  const { progress } = useWorkState(work, query);
  const snapshot = readStateOf(query) === 'ready' ? query?.data?.result.data : null;
  const review = snapshot?.work_id === work.key ? snapshot.observation?.collaboration?.review : null;
  if (!review?.id || !review.state) {
    return <ToneBadge tone={progress.tone} text={progress.text} testId={testId} />;
  }
  const word = statusWord(review.state, { language: i18n.language, field: 'state' });
  const reviewText = review.current_candidate === false
    ? t('plan.detail.earlierReview', { state: word.text })
    : review.current_candidate === true
      ? t('plan.detail.systemReview', { state: word.text })
      : t('plan.detail.unconfirmedReview', { state: word.text });
  return (
    <Space size={8} wrap data-testid={testId}>
      <ToneBadge
        tone={progress.tone}
        text={progress.kind === 'phase' ? t('plan.detail.reportedPhase', { phase: progress.text }) : progress.text}
      />
      <ToneBadge tone={review.current_candidate === true ? word.tone : 'neutral'} text={reviewText} />
    </Space>
  );
}
