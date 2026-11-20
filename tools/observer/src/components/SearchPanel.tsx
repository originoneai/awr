import { useQuery } from '@tanstack/react-query';
import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { useTranslation } from 'react-i18next';
import { overviewModelQueryOptions, projectsQueryOptions } from '../lib/queries';
import { searchSourcesFromReads } from '../lib/search-sources';
import { useRevocation } from '../lib/revocation';
import { SearchDialog } from './SearchDialog';

export { SearchDialog, SEARCH_RECENT_STORAGE_KEY, readRecentSearchKeys } from './SearchDialog';

interface SearchPanelProps {
  projectKey: string | null;
  renderTrigger: (open: (trigger: HTMLElement | null) => void, revoked: boolean) => ReactNode;
}

/** Existing list reads only. Opening the panel subscribes to the overview composition; closing disables it. */
export function SearchPanel({ projectKey, renderTrigger }: SearchPanelProps) {
  const { i18n } = useTranslation();
  const { revoked } = useRevocation();
  const [open, setOpen] = useState(false);
  const triggerRef = useRef<HTMLElement | null>(null);
  const projects = useQuery(projectsQueryOptions(revoked));
  const effectiveProject = projectKey ?? projects.data?.projects[0]?.key ?? null;
  const overview = useQuery({
    ...overviewModelQueryOptions(effectiveProject, revoked),
    enabled: open && Boolean(effectiveProject) && !revoked,
  });
  const sources = useMemo(() => searchSourcesFromReads({
    projects: projects.data,
    overview: overview.data,
    projectKey: effectiveProject,
    projectsFailed: projects.isError,
    overviewFailed: overview.isError,
  }), [effectiveProject, i18n.language, overview.data, overview.isError, projects.data, projects.isError]);

  const openPanel = useCallback((trigger: HTMLElement | null) => {
    if (open || revoked) return;
    triggerRef.current = trigger;
    setOpen(true);
  }, [open, revoked]);
  const closePanel = useCallback(() => setOpen(false), []);
  const restoreTrigger = useCallback(() => {
    if (triggerRef.current?.isConnected) triggerRef.current.focus();
    triggerRef.current = null;
  }, []);

  useEffect(() => {
    const onKeyDown = (event: globalThis.KeyboardEvent) => {
      if ((!event.metaKey && !event.ctrlKey) || event.key.toLocaleLowerCase() !== 'k') return;
      event.preventDefault();
      openPanel(document.activeElement instanceof HTMLElement ? document.activeElement : null);
    };
    window.addEventListener('keydown', onKeyDown);
    return () => window.removeEventListener('keydown', onKeyDown);
  }, [openPanel]);

  useEffect(() => {
    if (revoked) setOpen(false);
  }, [revoked]);

  return (
    <>
      {renderTrigger(openPanel, revoked)}
      {revoked ? null : (
        <SearchDialog
          open={open}
          sources={sources}
          loading={projects.isPending || (effectiveProject !== null && overview.isPending)}
          onClose={closePanel}
          afterClose={restoreTrigger}
        />
      )}
    </>
  );
}
