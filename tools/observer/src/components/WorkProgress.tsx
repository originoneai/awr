import { useTranslation } from 'react-i18next';
import { deriveWorkState } from '../core/observation';
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
  const { progress } = useWorkState(work, query);
  return <ToneBadge tone={progress.tone} text={progress.text} testId={testId} />;
}
