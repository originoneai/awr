import { useQueries, type UseQueryResult } from '@tanstack/react-query';
import { workSnapshotQueryOptions } from './queries';
import type { TeamReadResponse, TeamWork, WorkSnapshotData } from './types';

/** The part of a snapshot read the lists and the detail look at; any react-query result of it fits. */
export type SnapshotQuery = Pick<UseQueryResult<TeamReadResponse<WorkSnapshotData>, unknown>, 'data' | 'error' | 'isError' | 'isPending'>;

/** One snapshot read per listed work on the list cadence; the cache keys are shared with the work detail. */
export function useWorkSnapshots(project: string, works: TeamWork[], revoked: boolean): SnapshotQuery[] {
  return useQueries({
    queries: works.map((work) => workSnapshotQueryOptions(project, work.key, work.workstream_id, revoked, 'list')),
  });
}
