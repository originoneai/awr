import type {
  CountDetails,
  CoverageIssue,
  DerivedChain,
  DerivedTask,
  DerivedWait,
  OverviewInput,
  OverviewPeriod,
  OverviewStatusWaitingDerivation,
  OverviewViewSummary,
  ViewAgentSummary,
  ViewId,
  ViewMetric,
  ViewMetricKey,
  ViewUnit,
} from './types';
import {
  details,
  filterWaitsToTasks,
  isPrototypeInProgress,
  makeIssue,
  readItems,
  samePerson,
  uniqueIssues,
  validId,
  type PendingIndex,
} from './shared';

interface ChainResult {
  chains: DerivedChain[];
  chainCounts: { all: CountDetails<DerivedChain> };
}

function wide<T>(metric: ViewMetric<T>): ViewMetric<unknown> {
  return metric as ViewMetric<unknown>;
}

function periodBounds(period: OverviewPeriod | null | undefined): [number, number] | null {
  const start = period?.startUnixMs;
  const end = period?.endUnixMs;
  if (typeof start !== 'number' || !Number.isFinite(start) || start < 0
    || typeof end !== 'number' || !Number.isFinite(end) || end < start) return null;
  return [start, end];
}

/** Four prototype metrics per view, all built from the first slice's task and wait identities. */
export function deriveViews(
  input: OverviewInput,
  base: OverviewStatusWaitingDerivation,
  chainResult: ChainResult,
  pending: PendingIndex,
) {
  function metric<T>(key: ViewMetricKey, items: T[], issues: CoverageIssue[]): ViewMetric<T> {
    return { key, ...details(items, issues) };
  }

  function unknownView(keys: [ViewMetricKey, ViewMetricKey, ViewMetricKey, ViewMetricKey], reason: string): OverviewViewSummary {
    const issue = makeIssue('tasks', reason);
    return { metrics: [
      wide(metric(keys[0], [], [issue])), wide(metric(keys[1], [], [issue])),
      wide(metric(keys[2], [], [issue])), wide(metric(keys[3], [], [issue])),
    ] };
  }

  const options = input.options.views;
  if (options == null) {
    const views: Record<ViewId, OverviewViewSummary> = {
      me: unknownView(['verified-period', 'agents-running', 'waiting-by-me', 'pending-for-me'], 'view-options-unknown'),
      lead: unknownView(['verified-period', 'in-progress', 'waits', 'pending-for-me'], 'view-options-unknown'),
      department: unknownView(['projects', 'verified-period', 'cross-group-waits', 'pending-for-me'], 'view-options-unknown'),
      top: unknownView(['goals', 'verified-total', 'blockers', 'decisions-for-me'], 'view-options-unknown'),
    };
    return { views, coverageIssues: [makeIssue('tasks', 'view-options-unknown')] };
  }

  const memberGroup = new Map<string, string | null>();
  for (const member of readItems(input.members)) {
    const personId = validId(member.personId);
    if (personId != null) memberGroup.set(personId, validId(member.groupId));
  }
  const groupOfTask = (task: DerivedTask | null): string | null => {
    if (task == null) return null;
    return validId(task.groupId) ?? (task.owner ? memberGroup.get(task.owner.id) ?? null : null);
  };

  function projectScope(projectIds: readonly string[] | null | undefined) {
    if (projectIds == null) return { tasks: [] as DerivedTask[], issues: [makeIssue('tasks', 'view-project-scope-unknown')] };
    const allowed = new Set(projectIds);
    const issues = [...base.taskCounts.all.unknownItems];
    const tasks = base.tasks.filter((task) => {
      const projectId = validId(task.projectId);
      if (projectId == null) {
        issues.push(makeIssue('tasks', 'view-task-project-unknown', task.id));
        return false;
      }
      return allowed.has(projectId);
    });
    return { tasks, issues: uniqueIssues(issues) };
  }

  function periodItems(tasks: DerivedTask[], period: OverviewPeriod | null | undefined, name: string): CountDetails<DerivedTask> {
    const bounds = periodBounds(period);
    if (bounds == null) return details([], [makeIssue('tasks', `view-${name}-period-unknown`)]);
    const [start, end] = bounds;
    const issues: CoverageIssue[] = [];
    const items = tasks.filter((task) => {
      if (task.status !== 'verified') return false;
      const at = task.verifiedAtUnixMs;
      if (typeof at !== 'number' || !Number.isFinite(at) || at < 0) {
        issues.push(makeIssue('tasks', 'view-verified-time-unknown', task.id));
        return false;
      }
      return at >= start && at <= end;
    });
    return details(items, issues);
  }

  function periodMetric(tasks: DerivedTask[], current: OverviewPeriod | null | undefined,
    previous: OverviewPeriod | null | undefined, currentName: string, previousName: string,
    scopeIssues: CoverageIssue[] = []) {
    const currentItems = periodItems(tasks, current, currentName);
    const previousRaw = periodItems(tasks, previous, previousName);
    const previousPeriod = details(previousRaw.items, [...scopeIssues, ...previousRaw.unknownItems]);
    return {
      ...metric('verified-period', currentItems.items, [...scopeIssues, ...currentItems.unknownItems]),
      previousPeriod,
    };
  }

  function pendingMetric(
    key: 'pending-for-me' | 'decisions-for-me',
    personId: string | null | undefined,
    taskIds: ReadonlySet<string> | null,
    projectIds: ReadonlySet<string> | null,
    scopeIssues: CoverageIssue[],
  ) {
    if (validId(personId) == null) return metric(key, [], [makeIssue('tasks', 'view-person-unknown')]);
    const issues = [...scopeIssues];
    if (key === 'decisions-for-me') issues.push(...base.waitCounts.byKind.decision.unknownItems);
    else issues.push(...pending.countIssues);
    const items = (pending.byPerson.get(personId!) ?? []).filter((item) => {
      if (key === 'decisions-for-me' && (item.kind !== 'wait' || item.wait?.kind !== 'decision')) return false;
      if (item.kind === 'approval') {
        // Approvals are not task-bound: the me/lead/department queues scope them by their own
        // project. A missing project keeps the count unknown instead of dropping or guessing it.
        if (projectIds == null) return false;
        const projectId = item.approval == null ? null : validId(item.approval.projectId);
        if (projectId == null) {
          issues.push(makeIssue('approvals', 'view-approval-project-unknown'));
          return false;
        }
        return projectIds.has(projectId);
      }
      if (taskIds == null) return true;
      if (item.task?.id == null) {
        issues.push(makeIssue('tasks', 'view-pending-task-unknown'));
        return false;
      }
      return taskIds.has(item.task.id);
    });
    return metric(key, items, issues);
  }

  function unitItems(tasks: DerivedTask[], kind: 'project' | 'goal', catalog: readonly string[] | null | undefined) {
    const byId = new Map<string, ViewUnit>();
    const issues: CoverageIssue[] = [];
    if (catalog == null) issues.push(makeIssue('tasks', `view-${kind}-catalog-unknown`));
    const catalogIds = new Set<string>();
    for (const rawId of catalog ?? []) {
      const unitId = validId(rawId);
      if (unitId == null) {
        issues.push(makeIssue('tasks', `view-${kind}-catalog-entry-unknown`));
        continue;
      }
      catalogIds.add(unitId);
      byId.set(unitId, { id: unitId, kind, tasks: [] });
    }
    const remember = (unitId: string, task: DerivedTask) => {
      if (catalog != null && !catalogIds.has(unitId)) issues.push(makeIssue('tasks', `view-${kind}-outside-catalog`, task.id));
      const unit = byId.get(unitId) ?? { id: unitId, kind, tasks: [] };
      unit.tasks.push(task);
      byId.set(unitId, unit);
    };
    for (const task of tasks) {
      if (kind === 'project') {
        const projectId = validId(task.projectId);
        if (projectId == null) {
          issues.push(makeIssue('tasks', 'view-project-unknown', task.id));
          continue;
        }
        remember(projectId, task);
      } else {
        if (task.goalIds == null) {
          issues.push(makeIssue('tasks', 'view-task-goals-unknown', task.id));
          continue;
        }
        for (const rawGoalId of task.goalIds) {
          const goalId = validId(rawGoalId);
          if (goalId == null) {
            issues.push(makeIssue('tasks', 'view-goal-unknown', task.id));
            continue;
          }
          remember(goalId, task);
        }
      }
    }
    return { items: [...byId.values()], issues };
  }

  function crossGroupWaits(waits: DerivedWait[]) {
    const items: DerivedWait[] = [];
    const issues: CoverageIssue[] = [];
    for (const wait of waits) {
      const sourceGroup = groupOfTask(wait.task);
      if (sourceGroup == null) {
        issues.push(makeIssue('tasks', 'view-source-group-unknown', wait.task.id));
        continue;
      }
      if (wait.kind === 'external') {
        items.push(wait);
        continue;
      }
      const targetGroup = wait.kind === 'dependency'
        ? groupOfTask(wait.providerTask)
        : null;
      const targetGroups = wait.kind === 'dependency'
        ? [targetGroup]
        : wait.waitingOn.map((person) => memberGroup.get(person.id) ?? null);
      if (targetGroups.length === 0 || targetGroups.some((group) => group == null)) {
        issues.push(makeIssue('tasks', 'view-target-group-unknown', wait.task.id));
        if (!targetGroups.some((group) => group != null && group !== sourceGroup)) continue;
      }
      if (targetGroups.some((group) => group != null && group !== sourceGroup)) items.push(wait);
    }
    return { items, issues };
  }

  // Me: this week's verified, running Agents, waits on my tasks, and my pending queue.
  const meScope = options.me;
  const meProject = projectScope(meScope?.projectId == null ? null : [meScope.projectId]);
  const mePersonId = validId(meScope?.personId);
  const meTasks = mePersonId == null ? [] : meProject.tasks.filter((task) => samePerson(task.owner?.id, mePersonId));
  const mePersonIssues = mePersonId == null ? [makeIssue('tasks', 'view-person-unknown')] : [];
  const meVerified = mePersonId == null
    ? { ...metric('verified-period', [] as DerivedTask[], mePersonIssues), previousPeriod: details([] as DerivedTask[], mePersonIssues) }
    : periodMetric(meTasks, options.currentWeek, options.previousWeek, 'current-week', 'previous-week',
      [...meProject.issues, ...mePersonIssues]);
  const agentIssues = [...meProject.issues, ...mePersonIssues];
  const agentByName = new Map<string, ViewAgentSummary>();
  for (const task of meTasks) {
    // Running work counts, and so does a waiting task whose Agent is explicitly active: the task
    // waits on something else while the Agent keeps doing the wait-independent part, like the
    // prototype's task held up by an upstream review. Stalled, review, returned and claimed work
    // never counts, and a waiting task without an explicit active Agent is skipped silently
    // rather than treated as unknown.
    if (task.status !== 'running' && !(task.status === 'waiting' && task.agentActive === true)) continue;
    if (task.agentName == null) {
      agentIssues.push(makeIssue('tasks', 'view-agent-name-unknown', task.id));
      continue;
    }
    const agent = agentByName.get(task.agentName) ?? { agentName: task.agentName, tasks: [] };
    agent.tasks.push(task);
    agentByName.set(task.agentName, agent);
  }
  const meTaskIds = new Set(meTasks.map((task) => task.id).filter((value): value is string => value != null));
  // Pending work for the viewer spans the whole project, like the lead view: a reviewer or a
  // decider is waited on for other people's tasks too, not only for tasks they own.
  const meProjectTaskIds = new Set(meProject.tasks.map((task) => task.id).filter((value): value is string => value != null));
  const meProjectIds = meScope?.projectId == null ? null : new Set([meScope.projectId]);
  const meWaiting = filterWaitsToTasks(base.waits, meTaskIds).filter((wait) => {
    // The prototype's waiterOf assigns a self-addressed wait to the owner's Agent when one is
    // named. Such a wait is pending for the owner, not something the owner is waiting on others for.
    const ownerId = wait.task.owner?.id;
    const selfAddressed = ownerId != null && wait.waitingOn.length === 1
      && samePerson(wait.waitingOn[0].id, ownerId);
    return !(selfAddressed && Boolean(wait.task.agentName));
  });
  const me: OverviewViewSummary = { metrics: [
    wide(meVerified),
    wide(metric('agents-running', [...agentByName.values()], agentIssues)),
    wide(metric('waiting-by-me', meWaiting, [...meProject.issues, ...mePersonIssues, ...base.waitCounts.all.unknownItems])),
    wide(pendingMetric('pending-for-me', mePersonId, meProjectTaskIds, meProjectIds, meProject.issues)),
  ] };

  // Lead: this week's verified, prototype inProgress, all waits, and the lead's pending queue.
  const leadScope = options.lead;
  const leadProject = projectScope(leadScope?.projectId == null ? null : [leadScope.projectId]);
  const leadTasks = leadProject.tasks;
  const leadTaskIds = new Set(leadTasks.map((task) => task.id).filter((value): value is string => value != null));
  const leadProjectIds = leadScope?.projectId == null ? null : new Set([leadScope.projectId]);
  const leadWaits = filterWaitsToTasks(base.waits, leadTaskIds);
  const lead: OverviewViewSummary = { metrics: [
    wide(periodMetric(leadTasks, options.currentWeek, options.previousWeek, 'current-week', 'previous-week',
      leadProject.issues)),
    wide(metric('in-progress', leadTasks.filter(isPrototypeInProgress),
      [...leadProject.issues, ...base.coverage.sections.status.issues])),
    wide(metric('waits', leadWaits, [...leadProject.issues, ...base.waitCounts.all.unknownItems])),
    wide(pendingMetric('pending-for-me', leadScope?.personId, leadTaskIds, leadProjectIds, leadProject.issues)),
  ] };

  // Department: projects with tasks, this month's verified, cross-group waits, and pending items.
  const departmentScope = options.department;
  const departmentProject = projectScope(departmentScope?.projectIds);
  const departmentTasks = departmentProject.tasks;
  const departmentTaskIds = new Set(departmentTasks.map((task) => task.id).filter((value): value is string => value != null));
  const departmentProjectIds = departmentScope?.projectIds == null ? null : new Set(departmentScope.projectIds);
  const departmentWaits = filterWaitsToTasks(base.waits, departmentTaskIds);
  const projects = unitItems(departmentTasks, 'project', departmentScope?.projectIds);
  const crossGroup = crossGroupWaits(departmentWaits);
  const department: OverviewViewSummary = { metrics: [
    wide(metric('projects', projects.items, [...departmentProject.issues, ...projects.issues])),
    wide(periodMetric(departmentTasks, options.currentMonth, options.previousMonth, 'current-month', 'previous-month',
      departmentProject.issues)),
    wide(metric('cross-group-waits', crossGroup.items,
      [...departmentProject.issues, ...base.waitCounts.all.unknownItems, ...crossGroup.issues])),
    wide(pendingMetric('pending-for-me', departmentScope?.personId, departmentTaskIds, departmentProjectIds, departmentProject.issues)),
  ] };

  // Top: goals with tasks, all verified work (claimed comparison), blockers, and decisions for the viewer.
  const topIssues = [...base.taskCounts.all.unknownItems];
  const goals = unitItems(base.tasks, 'goal', options.top?.goalIds);
  const verified = base.taskCounts.byStatus.verified;
  const claimed = base.taskCounts.byStatus.claimed;
  const topVerified = { ...metric('verified-total', verified.items, verified.unknownItems), claimed };
  const top: OverviewViewSummary = { metrics: [
    wide(metric('goals', goals.items, [...topIssues, ...goals.issues])),
    wide(topVerified),
    wide(metric('blockers', chainResult.chains, chainResult.chainCounts.all.unknownItems)),
    wide(pendingMetric('decisions-for-me', options.top?.personId, null, null, [])),
  ] };

  const views: Record<ViewId, OverviewViewSummary> = { me, lead, department, top };
  const coverageIssues: CoverageIssue[] = [];
  for (const view of Object.values(views)) {
    for (const item of view.metrics) {
      coverageIssues.push(...item.unknownItems, ...item.previousPeriod?.unknownItems ?? [], ...item.claimed?.unknownItems ?? []);
    }
  }
  return { views, coverageIssues: uniqueIssues(coverageIssues) };
}
