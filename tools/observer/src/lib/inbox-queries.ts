import { useQueries, useQuery, type UseQueryResult } from '@tanstack/react-query';
import { useMemo } from 'react';
import { ApiError, apiGet, isAuthError } from './api';
import {
  aggregateInboxFreshness,
  buildInboxModel,
  isUsableInboxModel,
  type InboxModel,
  type InboxProjectRef,
} from './inbox-model';
import { POLL_INTERVAL_MS, isObservationReadRevoked, projectsQueryOptions, readQueryDefaults } from './queries';
import { useRevocation } from './revocation';
import type { TeamOverviewModelResponse } from './types';

/**
 * One project's composed overview model, read for the inbox. The shell and the inbox
 * page both subscribe through this hook, so the navigation pill, the bell and the
 * queue can never disagree. A single project's denial stays local to that project
 * (its pending items are simply unknown); only session-level failures revoke.
 */
export type InboxProjectRead =
  | TeamOverviewModelResponse
  | { ok: false; error: { code: string } };

export function inboxProjectReadQueryOptions(project: string, revoked = false) {
  return {
    queryKey: ['team', 'overview-model-inbox', project] as const,
    queryFn: async ({ signal, client }: {
      signal: AbortSignal;
      client: Parameters<typeof isObservationReadRevoked>[0];
    }): Promise<InboxProjectRead> => {
      if (revoked || isObservationReadRevoked(client)) throw new DOMException('Observation access revoked', 'AbortError');
      signal.throwIfAborted();
      try {
        const model = await apiGet<TeamOverviewModelResponse>(
          `/api/team/overview-model?project=${encodeURIComponent(project)}`,
        );
        signal.throwIfAborted();
        // The derivation is strict about its input; a reply that does not carry the
        // whole composed plane is a failed read for this project, never an app error.
        if (!isUsableInboxModel(model)) return { ok: false, error: { code: 'InvalidResponse' } };
        // An ordering the bridge could not compare never confirmed its own currency.
        return model.ordering === 'incomparable' ? { ok: false, error: { code: 'OrderingIncomparable' } } : model;
      } catch (error) {
        // Session-level failures revoke observation access globally. A single project's
        // Forbidden stays local: its pending items are unknown, the other projects stay.
        if (error instanceof ApiError && (error.code === 'Unauthenticated' || error.code === 'SessionExpired')) throw error;
        if (error instanceof DOMException && error.name === 'AbortError') throw error;
        return { ok: false, error: { code: error instanceof ApiError ? error.code : 'NetworkError' } };
      }
    },
    enabled: Boolean(project) && !revoked,
    ...readQueryDefaults,
    refetchInterval: POLL_INTERVAL_MS,
  };
}

export interface InboxRead {
  model: InboxModel;
  /** True while the first composition has not settled yet. */
  loading: boolean;
  /** True when reads ran and none of them succeeded. */
  allFailed: boolean;
  /** True when observation access was revoked or denied at session level. */
  forbidden: boolean;
  /** True when the project list itself failed. */
  projectsError: boolean;
  freshness: ReturnType<typeof aggregateInboxFreshness>;
  reads: UseQueryResult<InboxProjectRead>[];
}

/** The one inbox source shared by the shell indicators and the inbox page. */
export function useInboxModel(): InboxRead {
  const { revoked } = useRevocation();
  const projects = useQuery(projectsQueryOptions(revoked));
  const projectKeys = useMemo(
    () => (projects.data?.projects ?? []).map((project) => project.key),
    [projects.data],
  );
  const reads = useQueries({
    queries: projectKeys.map((key) => inboxProjectReadQueryOptions(key, revoked)),
  });

  return useMemo(() => {
    const usable = reads.flatMap((read) => (read.data && read.data.ok === true && isUsableInboxModel(read.data) ? [read.data] : []));
    const models = usable;
    const projectRefs: InboxProjectRef[] = (projects.data?.projects ?? []).map((project) => ({
      key: project.key,
      title: typeof project.title === 'string' && project.title.trim() !== '' ? project.title : null,
    }));
    const failedReads = reads.filter((read) => read.isError || (read.data !== undefined && read.data.ok === false)).length;
    const model = buildInboxModel({ models, projects: projectRefs, failedReads });
    const allFailedLocal = reads.length > 0 && reads.every((read) => read.isError || (read.data !== undefined && read.data.ok === false));
    // Every project denied the observation read: access was revoked for the viewer.
    const deniedEverywhere = allFailedLocal && reads.some((read) => read.data?.ok === false && read.data.error.code === 'Forbidden');
    const authDenied = projects.isError && isAuthError(projects.error)
      || reads.some((read) => read.isError && isAuthError(read.error));
    const loading = projects.isPending || (projectKeys.length > 0 && reads.some((read) => read.isPending));
    return {
      model,
      loading,
      allFailed: allFailedLocal,
      forbidden: revoked || authDenied || deniedEverywhere,
      projectsError: projects.isError,
      freshness: aggregateInboxFreshness(models),
      reads,
    };
  }, [reads, projects.data, projects.isPending, projects.isError, projects.error, revoked]);
}
