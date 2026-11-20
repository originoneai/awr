import i18n from '../i18n';
import { STATUS_LABELS } from '../i18n/human-language';
import type { OverviewMember, OverviewTask } from '../core/overview/types';
import type { SearchItem, SearchKind, SearchSource } from './search-index';
import type { TeamOverviewModelResponse, TeamProject, TeamProjectsResponse } from './types';

interface SearchReadInput {
  projects?: TeamProjectsResponse;
  overview?: TeamOverviewModelResponse;
  projectKey: string | null;
  /** True only when the viewer left the scope at all authorized projects. */
  allAuthorizedScope?: boolean;
  projectsFailed?: boolean;
  overviewFailed?: boolean;
  supplementalSources?: Partial<Record<'goal' | 'document' | 'lesson', SearchSource>>;
}

function recordedText(value: string | null | undefined): string | null {
  const text = value?.trim();
  return text ? text : null;
}

function itemKey(kind: SearchKind, project: string, id: string): string {
  return JSON.stringify([kind, project, id]);
}

function recordedTitle(value: string | null | undefined, internalId: string | null | undefined): string | null {
  const title = recordedText(value);
  return title && title !== internalId ? title : null;
}

function projectName(project: TeamProject | undefined, position: number): string {
  return recordedTitle(project?.title, project?.key) ?? i18n.t('shell.unnamedProject', { number: position });
}

function roleName(role: string | null | undefined, language: string): string | null {
  const words = role ? STATUS_LABELS[role] : undefined;
  if (!words) return null;
  return language === 'en-US' ? words[1] : words[0];
}

function taskItems(
  tasks: readonly OverviewTask[],
  members: readonly OverviewMember[],
  project: string,
  name: string,
): SearchItem[] {
  const memberNames = new Map<string, string>();
  for (const member of members) {
    const id = recordedText(member.personId);
    const displayName = recordedTitle(member.displayName, id);
    if (id && displayName) memberNames.set(id, displayName);
  }
  return tasks.flatMap((task, index) => {
    const id = recordedText(task.id);
    if (!id) return [];
    const owner = (task.ownerPersonId ? memberNames.get(task.ownerPersonId) : null)
      ?? i18n.t('shell.searchOwnerMissing');
    return [{
      key: itemKey('task', project, id),
      kind: 'task' as const,
      title: recordedTitle(task.title, id) ?? i18n.t('shell.searchUnnamed.task', { number: index + 1 }),
      meta: `${name} · ${owner}`,
      path: `/projects/${encodeURIComponent(project)}?tab=plan&work=${encodeURIComponent(id)}`,
    }];
  });
}

function memberItems(members: readonly OverviewMember[], project: string, name: string): SearchItem[] {
  return members.flatMap((member, index) => {
    const id = recordedText(member.personId);
    if (!id) return [];
    return [{
      key: itemKey('person', project, id),
      kind: 'person' as const,
      title: recordedTitle(member.displayName, id) ?? i18n.t('shell.searchUnnamed.person', { number: index + 1 }),
      meta: `${name} · ${i18n.t('shell.searchProjectMember')}`,
      path: `/members/${encodeURIComponent(id)}?scope=${encodeURIComponent(project)}`,
    }];
  });
}

/** Same-name rows remain distinguishable without exposing work keys, person IDs or other records. */
function distinguishDuplicateMeta(items: readonly SearchItem[]): SearchItem[] {
  const counts = new Map<string, number>();
  for (const item of items) {
    const group = JSON.stringify([item.kind, item.title, item.meta]);
    counts.set(group, (counts.get(group) ?? 0) + 1);
  }
  const positions = new Map<string, number>();
  return items.map((item) => {
    const group = JSON.stringify([item.kind, item.title, item.meta]);
    const position = (positions.get(group) ?? 0) + 1;
    positions.set(group, position);
    if ((counts.get(group) ?? 0) < 2) return { ...item };
    return { ...item, meta: `${item.meta} · ${i18n.t('shell.searchEntryNumber', { number: position })}` };
  });
}

function unread(kind: SearchKind): SearchSource {
  return { kind, coverage: 'unread', items: [] };
}

/**
 * Adapt only records returned by the existing list reads. There is deliberately no role- or
 * title-based visibility rule here: absence in the authorized reply is the permission boundary.
 */
export function searchSourcesFromReads({
  projects,
  overview,
  projectKey,
  allAuthorizedScope = false,
  projectsFailed = false,
  overviewFailed = false,
  supplementalSources,
}: SearchReadInput): SearchSource[] {
  const visibleProjects = projects?.projects ?? [];
  const selectedKey = projectKey ?? recordedText(visibleProjects[0]?.key);
  const selectedPosition = visibleProjects.findIndex((project) => project.key === selectedKey);
  const selectedProject = selectedPosition >= 0 ? visibleProjects[selectedPosition] : undefined;
  const selectedName = selectedKey
    ? projectName(selectedProject, selectedPosition >= 0 ? selectedPosition + 1 : 1)
    : i18n.t('shell.currentProject');
  const implicitProjectName = allAuthorizedScope && visibleProjects.length > 1 ? selectedName : undefined;
  const model = overview && selectedKey && overview.project === selectedKey ? overview : undefined;
  const taskSlice = model?.input?.tasks;
  const memberSlice = model?.input?.members;
  const tasks = taskSlice?.coverage === 'unread' ? [] : (taskSlice?.items ?? []);
  const members = memberSlice?.coverage === 'unread' ? [] : (memberSlice?.items ?? []);
  const language = i18n.language;

  const projectItems = visibleProjects.flatMap((project, index) => {
    const key = recordedText(project.key);
    if (!key) return [];
    return [{
      key: itemKey('project', key, key),
      kind: 'project' as const,
      title: projectName(project, index + 1),
      meta: roleName(project.role, language) ?? i18n.t('shell.searchAuthorizedProject'),
      path: `/projects/${encodeURIComponent(key)}`,
    }];
  });

  return [
    {
      kind: 'project',
      coverage: projects ? 'complete' : 'unread',
      items: distinguishDuplicateMeta(projectItems),
      failed: projectsFailed,
    },
    {
      kind: 'task',
      coverage: taskSlice?.coverage ?? 'unread',
      items: selectedKey ? distinguishDuplicateMeta(taskItems(tasks, members, selectedKey, selectedName)) : [],
      failed: overviewFailed,
      implicitProjectName,
    },
    {
      kind: 'person',
      coverage: memberSlice?.coverage ?? 'unread',
      items: selectedKey ? distinguishDuplicateMeta(memberItems(members, selectedKey, selectedName)) : [],
      failed: overviewFailed,
      implicitProjectName,
    },
    supplementalSources?.goal ?? unread('goal'),
    supplementalSources?.document ?? unread('document'),
    supplementalSources?.lesson ?? unread('lesson'),
  ];
}
