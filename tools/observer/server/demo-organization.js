'use strict';

const crypto = require('node:crypto');
const completeOrganization = require('../fixtures/demo-organization.json');

const DEMO_VARIANTS = new Set(['full', 'empty', 'single', 'restricted', 'partial', 'canonical']);
const DEMO_FUTURE_READ_OPS = new Set([
  'updates.list', 'update.inspect', 'goals.list', 'goal.inspect', 'lessons.list', 'lesson.inspect',
]);
const DAY = 86_400_000;
const WEEK = 7 * DAY;
const sha256 = (value) => crypto.createHash('sha256').update(value).digest('hex');
const clone = (value) => structuredClone(value);
const asObject = (value) => value && typeof value === 'object' && !Array.isArray(value);
const statusOf = (task) => task.prototype_status;
const DEMO_TIMESTAMP_KEY = /(?:_ms|_unix_ms|UnixMs)$/;
const completeSlice = (items) => ({ coverage: 'complete', items });

/** The demo clock's default offset lands the fixture "now" on the same weekday and time of
 * day: pages cut their "this week" windows on the viewer's calendar, and only whole-week
 * offsets keep the fixture's calendar weeks catching the same events as the prototype. */
function defaultDemoTimestampShiftMs() {
  return Math.round((Date.now() - completeOrganization.now_unix_ms) / WEEK) * WEEK;
}

function shiftDemoTimestamps(value, shift, key = null) {
  if (typeof value === 'number' && key !== null && DEMO_TIMESTAMP_KEY.test(key)) return value + shift;
  if (Array.isArray(value)) return value.map((item) => shiftDemoTimestamps(item, shift));
  if (!asObject(value)) return value;
  return Object.fromEntries(Object.entries(value).map(([childKey, item]) =>
    [childKey, shiftDemoTimestamps(item, shift, childKey)]));
}

class DemoReadError extends Error {
  constructor(code, message, status = 503) {
    super(message);
    this.code = code;
    this.status = status;
  }
}

function variantData(variant) {
  if (!DEMO_VARIANTS.has(variant)) throw new Error(`Unknown demo variant: ${variant}`);
  const data = clone(completeOrganization);
  if (variant === 'canonical') {
    // Canonical records the viewer's explicit all-projects grant. Visibility counts below
    // describe coverage; this separate authorization fact is what opens the top perspective.
    data.viewer.can_view_all_projects = true;
  }
  if (variant === 'empty') {
    for (const key of ['people', 'projects', 'tasks', 'goals', 'documents', 'updates', 'decisions',
      'project_questions', 'view_roles', 'handoffs', 'planning_candidates', 'activity', 'lessons',
      'pending', 'approvals', 'waits_between_groups']) data[key] = [];
    data.organization.departments = [];
    return data;
  }
  if (variant !== 'single') return data;
  const projectIds = new Set(['P-001']);
  const projects = data.projects.filter((project) => projectIds.has(project.project_id));
  const memberIds = new Set(projects.flatMap((project) => project.member_person_ids));
  // Approvals are canonical-fact shaped (OverviewApproval fields: projectId, recipientPersonId),
  // unlike the service-shaped collections around them, so project scoping reads either key.
  for (const key of ['projects', 'tasks', 'goals', 'documents', 'updates', 'decisions', 'project_questions',
    'handoffs', 'planning_candidates', 'activity', 'lessons', 'pending', 'approvals']) {
    data[key] = data[key].filter((item) => projectIds.has(item.project_id ?? item.projectId));
  }
  data.people = data.people.filter((person) => memberIds.has(person.person_id));
  data.organization.departments = data.organization.departments.filter((department) =>
    ['G-001', 'G-004'].includes(department.id));
  data.waits_between_groups = data.waits_between_groups.filter((wait) => {
    const task = data.tasks.find((item) => item.work_id === wait.work_id);
    return Boolean(task);
  });
  return data;
}

/** Monday-00:00 UTC week bounds of a moment; whole-week clock offsets keep them aligned
 * with every fixed-offset viewer's calendar week for the fixture's mid-day timestamps. */
function utcMondayWeek(nowUnixMs, weeksBefore = 0) {
  const date = new Date(nowUnixMs);
  const day = Date.UTC(date.getUTCFullYear(), date.getUTCMonth(), date.getUTCDate());
  const weekday = (date.getUTCDay() + 6) % 7;
  const start = day - (weekday + weeksBefore * 7) * DAY;
  return { startUnixMs: start, endUnixMs: start + WEEK - 1 };
}

function utcCalendarMonth(nowUnixMs, monthsBefore = 0) {
  const date = new Date(nowUnixMs);
  const start = Date.UTC(date.getUTCFullYear(), date.getUTCMonth() - monthsBefore, 1);
  return { startUnixMs: start, endUnixMs: Date.UTC(date.getUTCFullYear(),
    date.getUTCMonth() - monthsBefore + 1, 1) - 1 };
}

/**
 * The fully-known overview input: every fact the derived model consumes, taken straight
 * from the demo organization. The live reads cannot supply several of these planes yet
 * (structured external waits, acceptance evidence, submitted-result times, approvals),
 * so the `full` variant honestly reports them unknown; this canonical input is what the
 * same organization looks like once every read exists. The periods follow the demo clock
 * in UTC, so whole-week offsets move them with the facts they window.
 */
function canonicalOverviewInput(data) {
  const activity = data.tasks.flatMap((task) => task.timeline.map((entry) => ({
    id: entry.activity_id,
    taskId: task.work_id,
    action: 'session.checkpoint',
    actorId: `${task.owner_person_id}-agent`,
    atUnixMs: entry.at_unix_ms,
  })));
  return {
    tasks: completeSlice(data.tasks.map((task) => ({
      id: task.work_id,
      title: task.title,
      ownerPersonId: task.owner_person_id,
      agentName: task.agent_name,
      projectId: task.project_id,
      groupId: task.group_id,
      goalIds: task.goal_ids,
      commitments: [
        { id: `project-${task.project_id}`, title: task.project_id, kind: 'project' },
        ...task.goal_ids.map((goalId) => ({ id: goalId, title: goalId, kind: 'goal' })),
        ...(task.week_committed ? [{ id: `${task.project_id}-week-47`, title: 'Week 47 commitment', kind: 'project' }] : []),
      ],
      verifiedAtUnixMs: task.verified_at_unix_ms,
      acceptance: task.acceptance_final ? task.acceptance.map((criterion) => ({
        evidence: [{ passed: criterion.evidence_state === 'ok', current: true }],
      })) : [],
      report: task.report ? {
        completed: task.report.completed_declaration,
        reportedAtUnixMs: task.report.reported_at_unix_ms,
      } : null,
      submittedResultAtUnixMs: task.submitted_result_at_unix_ms,
      integrated: task.integrated,
      started: task.started,
      agentActive: task.agent_active,
      externalWait: task.wait?.kind === 'ext' ? {
        party: task.wait.waiting_on_name,
        what: task.wait.what,
        sinceUnixMs: task.wait.started_at_unix_ms,
      } : null,
    }))),
    dependencies: completeSlice(data.tasks.flatMap((task) => task.dependencies.map((dependency) => ({
      id: dependency.dependency_id,
      consumerTaskId: task.work_id,
      providerTaskId: dependency.provider_work_id,
      satisfied: dependency.satisfied,
      what: dependency.required_version_label,
      sinceUnixMs: dependency.since_unix_ms,
    })))),
    decisions: completeSlice(data.decisions.map((decision) => ({
      id: decision.decision_id,
      taskId: decision.work_id,
      state: decision.state,
      current: decision.current,
      question: decision.question,
      // Keep the short question as the card title and expose the separately recorded
      // focus description. Live replies without this optional fact use the page template.
      description: data.pending.find((item) => item.work_id === decision.work_id
        && item.title === decision.question && ['回答', '决定'].includes(item.kind))?.description ?? null,
      impact: decision.impact,
      allowedResponderIds: decision.allowed_responder_person_ids,
      createdAtUnixMs: decision.created_at_unix_ms,
    }))),
    reviewRounds: completeSlice(data.tasks.flatMap((task) => (task.delivery?.rounds || []).map((round) => ({
      id: round.round_id,
      taskId: task.work_id,
      roundIndex: round.round_index,
      state: round.state,
      current: round.current,
      independent: round.independent,
      authorPersonId: round.author_person_id,
      reviewerPersonId: round.reviewer_person_id,
      authorActorId: round.author_actor_id,
      reviewerActorId: round.reviewer_actor_id,
      openedAtUnixMs: round.opened_at_unix_ms,
    })))),
    handoffs: completeSlice(data.handoffs.map((handoff) => ({
      id: handoff.handoff_id,
      taskId: handoff.work_id,
      kind: handoff.kind,
      status: handoff.status,
      pending: handoff.pending,
      fromPersonId: handoff.from_person_id,
      toPersonId: handoff.to_person_id,
      updatedAtUnixMs: handoff.updated_at_unix_ms,
    }))),
    // The fixture's approvals are the canonical pending-approval facts the prototype's inbox
    // holds for 林悦 (the candidate lesson A-002) and 周宁 (the plan breakdown A-004). Each
    // maps one-to-one onto OverviewApproval; the fixture checks pin every one to its pending
    // record, its lesson or planning candidate, and its recipient and project.
    approvals: completeSlice(data.approvals.map((approval) => ({
      id: approval.id,
      kind: approval.kind,
      recipientPersonId: approval.recipientPersonId,
      projectId: approval.projectId,
      title: approval.title,
      sinceUnixMs: approval.sinceUnixMs,
    }))),
    members: completeSlice([...data.people, ...data.view_roles].map((member) => ({
      personId: member.person_id,
      displayName: member.display_name,
      groupId: member.group_id,
    }))),
    activity: completeSlice(activity),
    options: {
      nowUnixMs: data.now_unix_ms,
      stalledAfterDays: 1,
      views: {
        me: { personId: 'M-002', projectId: 'P-001' },
        lead: { personId: 'M-001', projectId: 'P-001' },
        department: { personId: 'M-010', projectIds: ['P-001', 'P-002', 'P-003'] },
        top: { personId: 'M-011', goalIds: ['GOAL-001', 'GOAL-002', 'GOAL-003'] },
        currentWeek: utcMondayWeek(data.now_unix_ms, 0),
        previousWeek: utcMondayWeek(data.now_unix_ms, 1),
        currentMonth: utcCalendarMonth(data.now_unix_ms, 0),
        previousMonth: utcCalendarMonth(data.now_unix_ms, 1),
      },
    },
  };
}

/** The demo snapshot's shared as-of triple, mirrored by the canonical overview model. */
function demoOverviewAsOf(projectId) {
  return {
    project_revision: '47',
    source_snapshot_id: `snapshot-${projectId}-47`,
    coordinator_epoch: `epoch-${projectId}-47`,
  };
}

/** Canonical project-directory facts no live read supplies yet: each ordinary project's
 * recorded lead (a person id the member directory resolves) and, for the prototype's
 * fourth row, a just-taken-over project with its takeover result facts. Demo-only
 * shapes: the project-lead metadata, AWR-PPD-052's group read and AWR-RCN-009's
 * takeover results replace them once those reads exist. */
const CANONICAL_PROJECT_LEADS = { 'P-001': 'M-001', 'P-002': 'M-005', 'P-003': 'M-007' };
const CANONICAL_TAKEOVER_PROJECT = {
  key: 'P-004',
  title: 'sample-cli',
  resultId: 'TK-001',
  workstreams: 4,
  verifiedTasks: 0,
  questions: 4,
  // The prototype's fourth row reads 刚刚: recorded half a minute before the demo clock.
  recordedBeforeNowMs: 30_000,
};

/** The canonical project directory: the fixture's ordinary projects with their recorded
 * lead and group display name, plus the taken-over sample project. Only the `canonical`
 * variant serves this shape; `full` keeps its three plain entries. */
function canonicalProjectList(data) {
  const groupNames = new Map(data.organization.departments.map((department) => [department.id, department.name]));
  const entries = data.projects.map((project) => ({
    key: project.key,
    title: project.title,
    role: data.viewer.role,
    leadPersonId: CANONICAL_PROJECT_LEADS[project.project_id] ?? null,
    groupName: groupNames.get(project.group_id) ?? null,
    // Recorded non-takeover: the canonical read says so, it does not leave the marker
    // unspecified (which would keep the page's takeover count honestly unknown).
    takeover: null,
  }));
  return [...entries, {
    key: CANONICAL_TAKEOVER_PROJECT.key,
    title: CANONICAL_TAKEOVER_PROJECT.title,
    role: data.viewer.role,
    takeover: {
      resultId: CANONICAL_TAKEOVER_PROJECT.resultId,
      recordedAtUnixMs: data.now_unix_ms - CANONICAL_TAKEOVER_PROJECT.recordedBeforeNowMs,
      verifiedTasks: CANONICAL_TAKEOVER_PROJECT.verifiedTasks,
      workstreams: CANONICAL_TAKEOVER_PROJECT.workstreams,
      questions: CANONICAL_TAKEOVER_PROJECT.questions,
    },
  }];
}

/** Facts for the existing perspective rules, derived from the canonical directory and
 * recorded viewer scope. No count is inferred from a requested or remembered perspective. */
function canonicalPerspectiveFacts(data) {
  const units = new Map(data.organization.departments.map((department) => [department.id, department]));
  const groupIds = new Set(data.projects.map((project) => project.group_id));
  const departmentIds = new Set(data.projects.map((project) => project.parent_department_id));
  const hierarchyKnown = groupIds.size > 0 && departmentIds.size > 0
    && [...groupIds].every((id) => units.has(id))
    && [...departmentIds].every((id) => units.has(id))
    && data.projects.every((project) => units.get(project.group_id)?.parent_id === project.parent_department_id);
  const viewerId = data.viewer.person_id;
  const supervisedGroups = new Set(data.projects.filter((project) => project.lead_person_id === viewerId)
    .map((project) => project.group_id));
  const memberGroups = new Set(data.projects.filter((project) => project.member_person_ids.includes(viewerId))
    .map((project) => project.group_id));
  return {
    supervisedGroupCount: supervisedGroups.size,
    memberGroupProjectCount: memberGroups.size,
    visibleProjectGroupCount: groupIds.size,
    visibleProjectDepartmentCount: departmentIds.size,
    canViewAllProjects: typeof data.viewer.can_view_all_projects === 'boolean'
      ? data.viewer.can_view_all_projects : null,
    directory: {
      groupsRegistered: hierarchyKnown ? true : null,
      departmentsRegistered: hierarchyKnown ? true : null,
    },
    // Pending counts remain the composition layer's facts. Unknown counts never open a view.
    pending: { me: null, lead: null, dept: null, top: null },
  };
}

const isoUtcDate = (unixMs) => new Date(unixMs).toISOString().slice(0, 10);

/** The canonical project period updates: one selected fixture update per project in
 * the page model's shape. The latest period is current for P-001/P-002 and historical
 * for P-003. Fixture dates are week-aligned strings that a whole-week clock offset
 * does not move, so the selected period is re-derived from the demo clock's own week:
 * it keeps its distance-in-weeks from the fixture's "now" week, whatever the offset.
 * Demo-only shape: AWR-PPD-010/011/012's update reads replace it once they exist. */
function canonicalPeriodUpdates(data, projectId) {
  const currentWeekStart = utcMondayWeek(data.now_unix_ms).startUnixMs;
  const fixtureWeekStart = utcMondayWeek(completeOrganization.now_unix_ms).startUnixMs;
  const items = data.updates.filter((update) => update.project_id === projectId)
    .sort((left, right) => right.starts_on.localeCompare(left.starts_on)
      || right.recorded_at_unix_ms - left.recorded_at_unix_ms)
    .slice(0, 1)
    .map((update) => {
      const startsOnMs = Date.parse(`${update.starts_on}T00:00:00Z`);
      const weeksBeforeFixtureNow = Math.round((fixtureWeekStart - startsOnMs) / WEEK);
      const startsAt = currentWeekStart - weeksBeforeFixtureNow * WEEK;
      return {
        id: update.update_id,
        startsOn: isoUtcDate(startsAt),
        endsOn: isoUtcDate(startsAt + WEEK - DAY),
        status: update.status,
        summary: update.summary,
        recordedAtUnixMs: update.recorded_at_unix_ms,
      };
    });
  return completeSlice(items);
}

/** Verbatim AWR-PPD-061 business duty presets matching Rust BusinessRole::action_ceiling. */
const DEMO_BUSINESS_ROLE_PRESETS = {
  action_ceilings: {
    administrator: [
      'work.read',
      'access.manage_project',
      'audit.read_project',
      'directory.manage_project',
    ],
    deliverer: [
      'work.read',
      'session.maintain_own',
      'delivery.finalize',
    ],
    developer: [
      'work.read',
      'session.maintain_own',
      'claim.manage_own',
      'execution.request_and_report_own',
      'planning.propose',
      'delivery.submit_and_request_review',
    ],
    observer: [
      'work.read',
    ],
    reviewer: [
      'work.read',
      'session.maintain_own',
      'review.decide',
    ],
    supervisor: [
      'work.read',
      'work.assign',
      'session.maintain_own',
      'planning.propose',
      'planning.edit_draft',
      'planning.approve',
      'planning.publish',
      'audit.read_project',
      'directory.manage_project',
    ],
  },
  explicit_combined_duties: true,
  omitted_on_update: 'preserve_existing_declaration',
  role_names_grant_authority: false,
  roles: [
    'observer',
    'developer',
    'reviewer',
    'supervisor',
    'deliverer',
    'administrator',
  ],
  semantics: 'membership_grants_intersect_declared_ceiling_then_one_live_delegation',
};

/** Demo-only organization projection for the canonical Members page. The synthetic fixture
 * deliberately uses one stable person ID per person across its three projects; real person IDs
 * remain project-scoped and no live organization directory is inferred from them. Duties and
 * project memberships are recorded fixture facts, not page-side guesses. */
function canonicalOrganizationDirectory(data) {
  const memberIds = new Set(data.people.map((member) => member.person_id));
  const visibleDepartmentIds = new Set([
    ...data.projects.flatMap((project) => [project.group_id, project.parent_department_id]),
    ...data.people.map((member) => member.group_id),
  ]);
  const allDepartments = data.organization.departments
    .filter((department) => visibleDepartmentIds.has(department.id))
    .map((department) => ({
      key: department.id, name: department.name, parentKey: department.parent_id ?? null,
    }));
  const departments = data.organization.departments.filter((department) =>
    data.people.some((member) => member.group_id === department.id));
  const departmentById = new Map(departments.map((department) => [department.id, {
    key: department.id, name: department.name, parentKey: department.parent_id ?? null,
  }]));
  return {
    configured: true,
    organization: data.organization.name,
    departments: [...departmentById.values()],
    allDepartments,
    updated_at_ms: data.now_unix_ms,
    members: data.people.filter((member) => memberIds.has(member.person_id)).map((member) => ({
      personId: member.person_id,
      name: member.display_name,
      available: true,
      departments: [departmentById.get(member.group_id)],
      duty: member.role,
      projects: data.projects.filter((project) => project.member_person_ids.includes(member.person_id))
        .map((project) => ({ key: project.key, name: project.title })),
    })),
  };
}

/**
 * Canonical department-level recorded facts carried alongside the demo portfolio read
 * (`variant === 'canonical'` only). Mirrors prototype VIEWS[2] from the fixture's own
 * tasks, goals, updates, group waits, pending decisions and lessons.
 */
function canonicalDepartmentFacts(data) {
  const departmentsById = new Map(data.organization.departments.map((d) => [d.id, d]));
  const peopleById = new Map([...data.people, ...(data.view_roles ?? [])].map((p) => [p.person_id, p]));
  const projectsById = new Map(data.projects.map((p) => [p.project_id, p]));
  const tasksById = new Map(data.tasks.map((t) => [t.work_id, t]));
  const goalsById = new Map(data.goals.map((g) => [g.goal_id, g]));

  const dependentsOf = (workId) => data.tasks.filter((t) =>
    (t.dependencies || []).some((d) => d.provider_work_id === workId));
  const behindCount = (workId, seen = new Set()) => {
    if (seen.has(workId)) return seen.size;
    seen.add(workId);
    for (const dep of dependentsOf(workId)) behindCount(dep.work_id, seen);
    return seen.size;
  };
  const wholeDays = (atUnixMs) => (typeof atUnixMs === 'number' && Number.isFinite(atUnixMs) && atUnixMs <= data.now_unix_ms
    ? Math.floor((data.now_unix_ms - atUnixMs) / DAY) : null);

  const shiftMs = data.now_unix_ms - completeOrganization.now_unix_ms;
  const curMonthBase = utcCalendarMonth(completeOrganization.now_unix_ms, 0);
  const prevMonthBase = utcCalendarMonth(completeOrganization.now_unix_ms, 1);
  const curMonth = { startUnixMs: curMonthBase.startUnixMs + shiftMs, endUnixMs: curMonthBase.endUnixMs + shiftMs };
  const prevMonth = { startUnixMs: prevMonthBase.startUnixMs + shiftMs, endUnixMs: prevMonthBase.endUnixMs + shiftMs };
  const inRange = (t, range) => statusOf(t) === 'verified'
    && typeof t.verified_at_unix_ms === 'number'
    && t.verified_at_unix_ms >= range.startUnixMs && t.verified_at_unix_ms <= range.endUnixMs;
  const currentVerified = data.tasks.filter((t) => inRange(t, curMonth)).map((t) => ({
    id: t.work_id,
    title: t.title,
    project_key: projectsById.get(t.project_id)?.key ?? t.project_id,
    project_title: projectsById.get(t.project_id)?.title ?? null,
    owner_name: peopleById.get(t.owner_person_id)?.display_name ?? null,
    verified_at_unix_ms: t.verified_at_unix_ms ?? null,
  }));
  const previousVerifiedCount = data.tasks.filter((t) => inRange(t, prevMonth)).length;

  const projectTitles = Object.fromEntries(data.projects.map((p) => [p.key, p.title]));
  const projectLeads = Object.fromEntries(data.projects.map((p) => [
    p.key,
    peopleById.get(CANONICAL_PROJECT_LEADS[p.project_id] ?? p.lead_person_id)?.display_name ?? '',
  ]).filter(([, name]) => Boolean(name)));

  const crossGroupWaits = (data.waits_between_groups ?? []).map((wait) => {
    const task = tasksById.get(wait.work_id);
    const proj = task ? projectsById.get(task.project_id) : null;
    const who = departmentsById.get(wait.source_group)?.name ?? wait.source_group;
    const on = wait.external_party ?? departmentsById.get(wait.target_group)?.name ?? wait.target_group;
    return {
      id: wait.wait_id,
      who,
      on,
      what: wait.what,
      kind: wait.kind === 'ext' ? 'external' : wait.kind,
      days: wholeDays(wait.started_at_unix_ms),
      started_at_unix_ms: wait.started_at_unix_ms ?? null,
      behind: wait.work_id ? behindCount(wait.work_id) : null,
      task_id: wait.work_id ?? null,
      task_title: task?.title ?? null,
      project_key: proj?.key ?? null,
    };
  }).sort((a, b) => (b.behind ?? -1) - (a.behind ?? -1) || (b.days ?? -1) - (a.days ?? -1));

  const deptQuestions = (data.project_questions ?? []).filter((q) =>
    q.state === 'open' && (q.allowed_responder_person_ids ?? []).includes('M-010'));
  const pending = (data.pending ?? []).filter((item) => item.recipient_role === 'department').map((item) => {
    const proj = projectsById.get(item.project_id);
    const matchingQuestion = deptQuestions.find((q) => q.project_id === item.project_id && q.question === item.title);
    const since = matchingQuestion?.created_at_unix_ms ?? item.created_at_unix_ms ?? null;
    return {
      id: matchingQuestion?.decision_id ?? item.pending_id,
      kind: item.kind === '决定' ? 'decision' : item.kind === '回答' ? 'answer' : 'decision',
      title: item.title,
      description: item.description ?? null,
      from: item.source ?? null,
      since_unix_ms: since,
      days: wholeDays(since),
      project_key: proj?.key ?? item.project_id,
      project_title: proj?.title ?? null,
      task_id: item.work_id ?? null,
      person_id: item.pending_id === 'A-006' ? 'M-004' : null,
      copy_text: item.copy_text ?? null,
      agent_guidance: item.agent_guidance ?? null,
    };
  });

  // Build the two department blocker chains from the fixture's open review queue and external wait.
  const chains = [];
  const reviewerQueue = data.tasks.filter((t) => statusOf(t) === 'review' && t.wait?.waiting_on_name === '许宁');
  const ordersProj = projectsById.get('P-001');
  const ordersGoal = goalsById.get('GOAL-001');
  const ordersWaitTask = tasksById.get('T-103');
  if (reviewerQueue.length > 0 && ordersProj) {
    const firstPending = pending[0];
    const queue = reviewerQueue.map((t) => ({
      title: t.title.split('：')[0].replace('提交失败的', '').replace('订单', ''),
      days: wholeDays(t.wait?.started_at_unix_ms),
    }));
    const inboxUrl = firstPending
      ? `/inbox?scope=${encodeURIComponent(firstPending.project_key)}&item=${encodeURIComponent(firstPending.id)}`
      : '/inbox';
    chains.push({
      id: 'dept-review-capacity',
      tab: '复核人手',
      footer: '周宁昨天请你决定：能不能多一位复核人。',
      cta: { label: '看这个决定', to: inboxUrl },
      nodes: [
        {
          id: 'dept-review-root',
          kind: 'root',
          root: true,
          kicker: '卡在这里',
          title: '只有一位复核人',
          owner_name: '许宁',
          queue,
          foot_note: `${queue.length} 件在排队，都等同一位复核人`,
          to: inboxUrl,
        },
        ...(ordersWaitTask ? [{
          id: 'dept-review-task',
          kind: 'task',
          kicker: departmentsById.get(ordersProj.group_id)?.name ?? '应用小组',
          title: ordersWaitTask.title,
          meta: '在等接口第 3 版',
          status: 'waiting',
          to: `/tasks/${encodeURIComponent(ordersWaitTask.work_id)}?scope=${encodeURIComponent(ordersProj.key)}`,
        }] : []),
        {
          id: 'dept-review-project',
          kind: 'project',
          kicker: '项目',
          title: ordersProj.title,
          meta: '本周承诺有风险',
          health: 'risk',
          to: `/projects/${encodeURIComponent(ordersProj.key)}`,
        },
        ...(ordersGoal ? [{
          id: 'dept-review-goal',
          kind: 'goal',
          kicker: '目标',
          title: ordersGoal.title,
          health: ordersGoal.health,
          foot_note: '有风险',
          to: `/goals/${encodeURIComponent(ordersGoal.goal_id)}?scope=${encodeURIComponent(ordersProj.key)}`,
        }] : []),
      ],
    });
  }
  const extTask = tasksById.get('T-303');
  const onboardProj = projectsById.get('P-003');
  const onboardGoal = goalsById.get('GOAL-003');
  if (extTask && onboardProj) {
    const extDays = wholeDays(extTask.wait?.started_at_unix_ms);
    const onboardUpdates = data.updates.filter((u) => u.project_id === onboardProj.project_id)
      .sort((a, b) => b.recorded_at_unix_ms - a.recorded_at_unix_ms);
    const staleDays = onboardUpdates[0] ? wholeDays(onboardUpdates[0].recorded_at_unix_ms) : null;
    chains.push({
      id: 'dept-external-account',
      tab: '甲方账号',
      footer: `郑岚 ${extDays ?? 9} 天前把这件事报给了最高层，还没有回答。`,
      cta: null,
      nodes: [
        {
          id: 'dept-ext-root',
          kind: 'root',
          root: true,
          kicker: '卡在这里',
          title: '甲方没给测试环境账号',
          meta: extDays !== null ? `${extDays} 天` : null,
          owner_name: '甲方',
          owner_external: true,
          to: `/tasks/${encodeURIComponent(extTask.work_id)}?scope=${encodeURIComponent(onboardProj.key)}`,
        },
        {
          id: 'dept-ext-project',
          kind: 'project',
          kicker: '项目',
          title: onboardProj.title,
          meta: staleDays !== null ? `整个项目停住，${staleDays} 天没有更新` : '整个项目停住',
          health: 'none',
          to: `/projects/${encodeURIComponent(onboardProj.key)}`,
        },
        ...(onboardGoal ? [{
          id: 'dept-ext-goal',
          kind: 'goal',
          kicker: '目标',
          title: onboardGoal.title,
          health: onboardGoal.health,
          foot_note: '没有更新',
          to: `/goals/${encodeURIComponent(onboardGoal.goal_id)}?scope=${encodeURIComponent(onboardProj.key)}`,
        }] : []),
      ],
    });
  }

  const lessonById = new Map((data.lessons ?? []).map((l) => [l.lesson_id, l]));
  const lessons = [
    { id: 'L-002', why: '被平台小组采用' },
    { id: 'L-003', why: '本月新增' },
  ].flatMap(({ id, why }) => {
    const lesson = lessonById.get(id);
    if (!lesson) return [];
    return [{
      id: lesson.lesson_id,
      title: lesson.title,
      stage: lesson.stage,
      author_name: peopleById.get(lesson.author_person_id)?.display_name ?? null,
      why,
      used: lesson.usage_records.length,
      passed: lesson.usage_records.filter((r) => r.outcome === 'passed').length,
      not_applicable: lesson.usage_records.filter((r) => r.outcome === 'not_applicable').length,
    }];
  });

  const deptUpdate = data.projects.length === 3 ? {
    id: null,
    health: 'risk',
    sentence: '3 个项目里 1 个有风险：订单协作平台只有一位复核人，排着 3 件。另有 1 个项目 12 天没有更新。',
    author_person_id: 'M-010',
    author_name: '部门负责人',
    written_at_unix_ms: data.now_unix_ms - 2 * DAY,
  } : null;

  return {
    department_name: departmentsById.get('G-004')?.name ?? null,
    viewer_person_id: 'M-010',
    department_update: deptUpdate,
    project_titles: projectTitles,
    project_leads: projectLeads,
    monthly_verified: {
      current: { count: currentVerified.length, items: currentVerified },
      previous: { count: previousVerifiedCount },
    },
    cross_group_waits: crossGroupWaits,
    pending,
    chains,
    lessons,
    usage: data.projects.length === 3 ? {
      collected_groups: ['应用小组'],
      missing_groups: ['平台小组', '交付小组'],
      by_project: {},
    } : {
      collected_groups: [],
      missing_groups: [...new Set(data.projects.map((p) => departmentsById.get(p.group_id)?.name).filter(Boolean))],
      by_project: {},
    },
  };
}

function createDemoOrganization({ variant = 'full',
  timestampShiftMs = defaultDemoTimestampShiftMs() } = {}) {
  if (!Number.isInteger(timestampShiftMs)) throw new Error('Demo timestamp offset must be an integer');
  const data = shiftDemoTimestamps(variantData(variant), timestampShiftMs);
  const revokedConnections = new Set();
  const taskById = new Map(data.tasks.map((task) => [task.work_id, task]));
  const projectById = new Map(data.projects.map((project) => [project.project_id, project]));
  const personById = new Map(data.people.map((person) => [person.person_id, person]));
  const goalById = new Map(data.goals.map((goal) => [goal.goal_id, goal]));
  const lessonById = new Map(data.lessons.map((lesson) => [lesson.lesson_id, lesson]));
  const updateById = new Map(data.updates.map((update) => [update.update_id, update]));

  function project(projectId) {
    const found = projectById.get(projectId);
    if (!found) throw new DemoReadError('Forbidden', 'access denied', 403);
    return found;
  }

  function task(workId, projectId) {
    const found = taskById.get(workId);
    if (!found || found.project_id !== projectId) throw new DemoReadError('Forbidden', 'access denied', 403);
    return found;
  }

  function person(personId, displayName) {
    const found = personById.get(personId);
    return {
      person_id: personId,
      display_name: found?.display_name ?? displayName ?? null,
    };
  }

  function actor(personId, displayName) {
    return { ...person(personId, displayName), actor_id: `${personId}-agent`, client_id: `${personId}-client` };
  }

  function viewerIdentity() {
    return {
      actor_id: data.viewer.actor_id,
      client_id: data.viewer.client_id,
      role: data.viewer.role,
      business_roles: [...data.viewer.business_roles],
    };
  }

  function contractHash(workId) {
    return sha256(`demo-contract:${workId}`);
  }

  function sourceSnapshot(projectId) {
    return `snapshot-${projectId}-47`;
  }

  function asOf(projectId) {
    return {
      at_unix_ms: data.now_unix_ms,
      project_revision: '47',
      source_snapshot_id: sourceSnapshot(projectId),
    };
  }

  function envelope(projectId, query, extra = {}) {
    const selectedTask = query.work_id ? taskById.get(query.work_id) : null;
    const workstreamId = query.workstream_id ?? selectedTask?.workstream_id;
    return {
      protocol_version: 1,
      scope_id: 'main',
      coordinator_epoch: `epoch-${projectId}-47`,
      project_status: 'active',
      source_snapshot_id: sourceSnapshot(projectId),
      project_revision: '47',
      selection_basis: query.work_id ? 'work' : query.workstream_id ? 'explicit' : 'project',
      ...(workstreamId ? { workstream_id: workstreamId, authority_version: '1' } : {}),
      ...extra,
    };
  }

  function reply(projectId, query, value) {
    return { ...envelope(projectId, query), data: value };
  }

  function page(items, query, defaultLimit = 20) {
    const limit = query.limit ?? defaultLimit;
    let offset = 0;
    if (query.cursor) {
      const match = /^demo-offset-([1-9][0-9]*)$/.exec(query.cursor);
      if (!match) throw new DemoReadError('CursorExpired', 'cursor expired', 409);
      offset = Number(match[1]);
    }
    const selected = items.slice(offset, offset + limit);
    const next = offset + limit < items.length ? `demo-offset-${offset + limit}` : null;
    return { items: selected, total: items.length, next_cursor: next, truncated: next !== null, offset };
  }

  function guard(projectId, query) {
    if (variant === 'restricted') {
      if (projectId === 'P-003' || ['documents.list', 'sources.inspect', 'audit.development', 'audit.requests'].includes(query.op)) {
        throw new DemoReadError('Forbidden', 'access denied', 403);
      }
    }
    if (variant === 'partial') {
      if (projectId === 'P-001' && query.op === 'work.list' && query.workstream_id === 'WS-002'
        || projectId === 'P-001' && query.op === 'work.graph' && query.workstream_id === 'WS-002'
        || projectId === 'P-001' && ['planning.candidates', 'audit.development'].includes(query.op)
        || projectId === 'P-001' && query.op === 'work.snapshot' && query.work_id === 'T-102') {
        throw new DemoReadError('Unavailable', 'read temporarily unavailable', 503);
      }
    }
  }

  function workstreams(projectId, query) {
    const streams = project(projectId).workstreams;
    const result = page(streams, query, 50);
    return {
      items: result.items.map(({ id, external_key, title, authority_version }) =>
        ({ id, external_key, title, authority_version })),
      total: result.total,
      next_cursor: result.next_cursor,
    };
  }

  function latestRound(task) {
    return task.delivery?.rounds.at(-1) ?? null;
  }

  function artifactText(task, round) {
    const checks = task.delivery?.checks.map((check) =>
      `• ${check.check}：${check.outcome === 'passed' ? '已通过' : '未通过'}`).join('\n') || '• 还没有检查记录';
    const criteria = task.acceptance.map((criterion) => `• ${criterion.text}：${criterion.evidence_detail}`).join('\n');
    return `${task.title}\n\n${task.report?.summary || task.one_sentence}\n\n完成标准\n${criteria}\n\n检查记录\n${checks}\n\n复核记录\n${round.reason_first_line}\n`;
  }

  function artifactDigest(task, round) {
    return sha256(artifactText(task, round));
  }

  function runtimeState(task) {
    if (statusOf(task) === 'verified') return 'completed';
    if (['ready', 'planned'].includes(statusOf(task))) return 'unclaimed';
    return 'claimed';
  }

  function phase(task) {
    return {
      verified: 'delivered', claimed: 'completed', returned: 'changes_requested', review: 'ready_for_review',
      running: 'in_progress', stalled: 'waiting_for_answer', waiting: 'blocked', ready: 'ready', planned: 'planned',
    }[statusOf(task)] || 'unknown';
  }

  function progress(task) {
    if (!task.report) return null;
    const round = latestRound(task);
    return {
      phase: phase(task),
      summary: task.report.summary,
      completed: task.report.completed,
      blockers: task.report.blockers,
      tests: task.report.tests,
      artifacts: [
        ...task.report.artifacts,
        ...(round ? [{ label: 'review round', reference: round.round_id }] : []),
      ],
      origin: {
        actor_id: `${task.owner_person_id}-agent`,
        actor_name: task.report.reporter,
        client_id: `${task.owner_person_id}-client`,
        session_id: `session-${task.work_id.toLowerCase()}`,
      },
      provenance: 'caller_declared',
      reported_at_unix_ms: task.report.reported_at_unix_ms,
      reported_contract_hash: contractHash(task.work_id),
      recorded_project_revision: '47',
      report_order_basis: 'recorded_project_revision',
      contract_matches_current: true,
      stale: false,
      next_action: task.next_step,
      checkpoint_id: `checkpoint-${task.work_id.toLowerCase()}`,
      client_observed_at_unix_ms: null,
    };
  }

  function integration(task) {
    if (!task.delivery) return null;
    const round = latestRound(task);
    return {
      id: round ? `integration-${round.round_id}` : `integration-${task.work_id.toLowerCase()}`,
      state: task.integrated ? 'confirmed' : round?.state === 'approved' ? 'dispatched' : 'prepared',
      reviewed_execution_id: round?.execution_id ?? null,
      current_binding: task.integrated ? true : round?.current ?? true,
      target_changed: false,
    };
  }

  function summary(task) {
    const report = progress(task);
    const at = task.report?.reported_at_unix_ms ?? data.now_unix_ms;
    const owner = person(task.owner_person_id);
    const blockers = task.report?.blockers ?? [];
    const deliveryIntegration = integration(task);
    const hasOutcome = report !== null || deliveryIntegration !== null;
    const hasRuntime = !['ready', 'planned'].includes(statusOf(task));
    return {
      version: 1,
      owner: { value: owner, as_of_unix_ms: data.now_unix_ms, reason: null },
      phase: { value: phase(task), as_of_unix_ms: at, reason: null },
      runtime: hasRuntime
        ? { value: runtimeState(task), as_of_unix_ms: data.now_unix_ms, reason: null }
        : { value: null, as_of_unix_ms: null, reason: 'no_runtime' },
      outcome: {
        value: hasOutcome ? {
          summary: report?.summary ?? null,
          completed: report?.completed ?? null,
          integration: deliveryIntegration,
        } : null,
        as_of_unix_ms: hasOutcome ? deliveryIntegration ? data.now_unix_ms : at : null,
        reason: hasOutcome ? null : 'no_reported_outcome',
      },
      execution: task.agent_name ? {
        value: {
          session_state: task.agent_active ? 'active' : 'ended',
          last_checkpoint_at_unix_ms: task.report?.reported_at_unix_ms ?? null,
        },
        as_of_unix_ms: data.now_unix_ms,
        reason: null,
      } : { value: null, as_of_unix_ms: null, reason: 'no_session' },
      blocker: {
        value: blockers.length ? { text: blockers[0] } : null,
        as_of_unix_ms: blockers.length ? at : null,
        reason: blockers.length ? null : 'no_reported_blocker',
      },
      next_action: { value: task.next_step, as_of_unix_ms: at, reason: null },
      report: { value: report, as_of_unix_ms: report ? at : null, reason: report ? null : 'no_report' },
      historical_responsibility: owner.display_name,
    };
  }

  function workList(projectId, query) {
    let tasks = data.tasks.filter((item) => item.project_id === projectId
      && (!query.workstream_id || item.workstream_id === query.workstream_id));
    if (query.op === 'work.search') {
      const needle = query.search.toLocaleLowerCase();
      tasks = tasks.filter((item) => item.title.toLocaleLowerCase().includes(needle));
    }
    const result = page(tasks, query);
    return reply(projectId, query, {
      items: result.items.map((item) => ({
        work_id: item.work_id,
        external_key: item.external_key,
        title: item.title,
        workstream_id: item.workstream_id,
        contract_hash: contractHash(item.work_id),
        summary: summary(item),
      })),
      total: result.total,
      next_cursor: result.next_cursor,
      truncated: result.truncated,
      execution_authorized: false,
      as_of: asOf(projectId),
    });
  }

  function workGraph(projectId, query) {
    const tasks = data.tasks.filter((item) => item.project_id === projectId
      && (!query.workstream_id || item.workstream_id === query.workstream_id));
    const result = page(tasks, query);
    return reply(projectId, query, {
      items: result.items.map((item) => ({
        work_id: item.work_id,
        external_key: item.external_key,
        title: item.title,
        state: runtimeState(item),
        required_dependencies: item.dependencies.map((dependency) => dependency.provider_work_id),
        dependencies_complete: true,
        owner: person(item.owner_person_id).display_name,
        last_event_at: item.report?.reported_at_unix_ms ?? null,
        claims: item.agent_active ? [{ state: 'active', owner: item.agent_name }] : [],
      })),
      total: result.total,
      next_cursor: result.next_cursor,
      truncated: result.truncated,
      execution_authorized: false,
      as_of: asOf(projectId),
    });
  }

  function navigation(task) {
    if (task.assignment) return 'handoff_required';
    if (task.wait?.kind === 'ext') return 'waiting_external';
    if (task.dependencies.some((dependency) => !dependency.satisfied)) return 'waiting_dependency';
    if (task.agent_active) return 'resume';
    if (['ready', 'planned'].includes(statusOf(task))) return 'prepare';
    return 'observe';
  }

  function workNext(projectId, query) {
    const mine = data.tasks.filter((item) => item.project_id === projectId
      && item.owner_person_id === data.viewer.person_id && statusOf(item) !== 'verified');
    const limited = variant === 'partial' && projectId === 'orders' ? mine.slice(0, 3) : mine;
    const result = page(limited, query);
    return reply(projectId, query, {
      identity: { ...viewerIdentity() },
      items: result.items.map((item) => ({
        navigation: navigation(item),
        work_id: item.work_id,
        workstream_id: item.workstream_id,
        state: item.started ? 'claimed' : null,
        title: item.title,
        responsibility: { relation: 'owned_by_me', owner: data.viewer.person_id, version: '1' },
        next_query: { protocol_version: 1, op: item.agent_active ? 'work.observe' : 'work.prepare',
          work_id: item.work_id, workstream_id: item.workstream_id },
      })),
      resume: mine.filter((item) => item.agent_active).map((item) => ({
        work_id: item.work_id,
        title: item.title,
        session_id: `session-${item.work_id.toLowerCase()}`,
        session_version: '1',
        next_query: { protocol_version: 1, op: 'work.observe', work_id: item.work_id, workstream_id: item.workstream_id },
      })),
      next_cursor: variant === 'partial' && projectId === 'orders' ? 'demo-offset-3' : result.next_cursor,
      next_action: null,
      recheck_on: ['responsibility', 'dependency', 'review', 'permission'],
      execution_authorized: false,
    });
  }

  function workInbox(projectId, query) {
    const pending = data.pending.filter((item) => item.project_id === projectId && item.work_id
      && item.recipient_person_id === data.viewer.person_id);
    const result = page(pending, query);
    return reply(projectId, query, {
      identity: { actor_id: data.viewer.actor_id, client_id: data.viewer.client_id,
        role: data.viewer.role, business_roles: data.viewer.business_roles },
      items: result.items.map((item) => {
        const work = task(item.work_id, projectId);
        const nextQuery = { protocol_version: 1, op: 'work.observe', work_id: work.work_id, workstream_id: work.workstream_id };
        return {
          item_key: sha256(`inbox:${item.pending_id}`),
          work_id: work.work_id,
          workstream_id: work.workstream_id,
          contract_hash: contractHash(work.work_id),
          title: item.title,
          title_truncated: false,
          next_query: nextQuery,
          guidance: {
            code: item.kind === '接任务' ? 'handoff_required' : item.kind === '确认经验' ? 'observe' : 'held',
            when: item.source,
            because: item.basis.map((entry) => entry.text),
            action: { op: nextQuery.op, query: nextQuery, note: item.agent_guidance },
            recheck_on: 'Read the current task again after the person answers in their own Agent',
          },
        };
      }),
      next_cursor: result.next_cursor,
      next_action: 'Follow the related read; business handling stays in the person\'s own Agent',
      refresh_from_first_page_on_change: true,
      state_basis: 'current_persistent_facts',
      deduplicate_by: 'item_key',
      execution_authorized: false,
    });
  }

  function documentsForTask(task) {
    return data.documents.filter((document) => document.project_id === task.project_id
      && document.used_task_ids.includes(task.work_id));
  }

  function snapshot(projectId, query) {
    const work = task(query.work_id, projectId);
    const round = latestRound(work);
    const contract = contractHash(work.work_id);
    const digest = round ? artifactDigest(work, round) : null;
    const completionId = statusOf(work) === 'verified' && round ? `completion-${round.round_id}` : null;
    const integrationId = work.integrated && round ? `integration-${round.round_id}` : null;
    const runtime = ['ready', 'planned'].includes(statusOf(work)) ? null : {
      state: runtimeState(work), work_version: '1', last_fence: '1', recovery_blocked: false,
      selected_completion_id: completionId,
    };
    const docs = documentsForTask(work);
    const report = progress(work);
    const collaboration = {
      acceptance_inferred: false,
      delivery_required: Boolean(work.delivery),
      candidate: work.delivery && round ? {
        current: true, digest: sha256(`candidate:${round.round_id}`),
        required_checks: work.delivery.checks.map((check) => check.check), selection_version: '1',
      } : null,
      claim_present: Boolean(work.agent_active),
      facts_truncated: false,
      review: round ? {
        id: round.round_id, state: round.state, current_candidate: round.current,
        bundle_hash: round.bundle_hash, author_actor_id: round.author_actor_id,
      } : null,
      integration: integrationId ? {
        id: integrationId, state: 'confirmed', current_binding: true, target_changed: true,
        reviewed_execution_id: round.execution_id,
      } : null,
      publication: work.integrated && round ? {
        id: `publication-${round.round_id}`, phase: 'confirmed', metadata_revision: '1', basis: 'recorded_publication_phase',
      } : null,
      source_state: work.delivery ? {
        basis: 'current_physical_source', cursor_current: true, history_truncated: false,
        metadata_revision: '1', observation: 'observed', pending_publication_id: null,
        projection_current: true, query_started_at_unix_ms: data.now_unix_ms,
        source_fingerprint: `sha256:${sha256(`source:${work.work_id}`)}`, source_synchronized: true,
        query: { protocol_version: 1, op: 'delivery.source.status', work_id: work.work_id },
      } : null,
      source_synchronized: true,
      verification: work.delivery?.checks.map((check, index) => ({
        check: check.check, outcome: check.outcome, run_id: `check-${work.work_id.toLowerCase()}-${index + 1}`,
      })) ?? [],
      recovery: { claim_binding_changed: false, expired_claim: false, unknown_resource: false, unsettled_execution: false },
    };
    const observation = {
      work_id: work.work_id,
      workstream_id: work.workstream_id,
      contract_hash: contract,
      definition_state: 'enabled',
      observed_at_unix_ms: data.now_unix_ms,
      last_activity_at_unix_ms: work.report?.reported_at_unix_ms ?? work.timeline[0]?.at_unix_ms ?? null,
      automatic_resume: false,
      observation_scope: 'current_work_ownership',
      execution_authorized: false,
      responsibility: {
        owner: work.owner_person_id,
        owner_person_id: work.owner_person_id,
        owner_name: person(work.owner_person_id).display_name,
        relation: work.owner_person_id === data.viewer.person_id ? 'owned_by_me' : 'owned_by_other',
        current_executor: work.agent_name ? { kind: 'agent_run', person_id: work.owner_person_id, agent_id: `${work.owner_person_id}-agent` } : null,
        current_executor_matches_client: false,
        independent_reviewer: round?.reviewer_person_id ? person(round.reviewer_person_id, round.reviewer_display_name) : null,
        collaborators: [], pending: work.assignment?.handoff_id ?? null, version: '1',
      },
      runtime,
      session: work.agent_name ? {
        id: `session-${work.work_id.toLowerCase()}`, actor_id: `${work.owner_person_id}-agent`,
        actor_name: work.agent_name, client_id: `${work.owner_person_id}-client`, state: 'active',
      } : null,
      claim: work.agent_active ? {
        id: round?.claim_id ?? `claim-${work.work_id.toLowerCase()}`, state: 'active', lease_live: true,
        expires_at_unix_ms: data.now_unix_ms + 60 * 60 * 1000, epoch_matches_current: true,
      } : null,
      execution: round ? {
        execution_id: round.execution_id, claim_id: round.claim_id, session_id: `session-${work.work_id.toLowerCase()}`,
        state: 'succeeded', execution_version: '1', fence: '1', contract_hash: contract,
        contract_matches_current: true, epoch_matches_current: true, execution_coordinator_epoch: `epoch-${projectId}-47`,
        execution_authorized: false, automatic_resume: false, recovery_blocked: false, recovery_cause: 'none',
        effects_settled: true, terminal_reported: true, cancel_requested: false,
        artifact_verified: Boolean(completionId), artifact_verification_basis: completionId ? 'current_completion_readable_artifact_digest' : null,
        attestation_authority: false, owned_by_client: false, lease_live: Boolean(work.agent_active),
        latest_receipt: null, receipt_details_available: false, reconciliation_authority: false,
        previous_epoch_recovery_available: false, previous_epoch_review_required: false,
        settlement_basis: 'caller_asserted', settlement_scope: 'admitted_workspace_paths',
        controlled_confirmation_available: false, terminal_reporting: null,
        reconciliation_coverage: { basis: 'configured_authority', configuration_guidance: null,
          live_access_not_guaranteed: true, status: 'covered' },
        origin: { actor_id: round.author_actor_id, actor_name: work.agent_name,
          client_id: `${work.owner_person_id}-client`, session_id: `session-${work.work_id.toLowerCase()}`,
          model: null, model_basis: 'not_recorded_at_execution' },
        work_id: work.work_id,
      } : null,
      session_execution: null,
      progress: report,
      reporting: {
        version: 1, stale_after_ms: 900000, client_reported_at_unix_ms: work.report?.reported_at_unix_ms ?? null,
        client_stale: false, billing_collected: false, usage_aggregation: 'none', provenance: 'caller_declared',
        model_basis: 'latest_session_declaration; not execution-time attribution',
        client_origin: report?.origin ?? null,
      },
      collaboration,
      pr_deliveries: [],
      dependencies_ready_advisory: work.dependencies.every((dependency) => dependency.satisfied),
      checkpoint: report ? {
        id: report.checkpoint_id, created_at_unix_ms: report.reported_at_unix_ms,
        contract_matches_current: true, next_action: work.next_step, open_loops: work.report?.blockers ?? [],
      } : null,
      waiting_user: work.wait?.kind === 'decision',
      model: null,
      usage: null,
      missing: {
        execution_receipt: round ? null : 'not_recorded', model: 'client_capability_unknown',
        progress: report ? null : 'not_recorded', usage: 'client_collection_unsupported',
      },
      last_participation: report ? {
        actor_id: report.origin.actor_id, actor_name: report.origin.actor_name, client_id: report.origin.client_id,
        session_id: report.origin.session_id, at_unix_ms: report.reported_at_unix_ms,
        basis: 'latest_recorded_session_activity', recorded_project_revision: '47', state: 'active',
      } : null,
      guidance: { code: 'task_intake', when: 'observe_only', because: ['read-only observation'],
        action: { op: 'work.next', note: work.next_step }, recheck_on: 'recorded facts change' },
      client: { product: work.agent_name, version: null, model: null,
        capabilities: { model: 'unknown', progress: work.report ? 'supported' : 'unsupported', usage: 'unsupported' } },
      adopted_dependencies: [],
    };
    const visibleContract = {
      codec: 'awr-team-contract-v4', work_id: work.work_id, external_key: work.external_key,
      goals: work.goal_ids.map((goalId) => goalById.get(goalId)?.title ?? goalId),
      acceptance: work.acceptance.map((criterion) => criterion.text),
      required_dependencies: work.dependencies.map((dependency) => dependency.provider_work_id),
      hard_rules: project(projectId).rules.map((rule) => rule.text),
      scope_paths: [`docs/${projectId}/`],
      verification_requirements: work.delivery?.checks.map((check) => check.check) ?? [],
      completion_policy: 'caller_managed_execution_and_simulated_member_review',
      execution_settlement: { mode: 'independent_workspace_v2', workspace_id: `${projectId}-synthetic` },
    };
    return reply(projectId, query, {
      work_id: work.work_id,
      contract_hash: contract,
      context_hash: sha256(`context:${work.work_id}`),
      context_hash_protocol: 'awr-team-workstream-context-v1',
      definition_state: 'enabled',
      dependency_export_unavailable: false,
      execution_admission: 'not_evaluated',
      ownership_version: '1',
      context_complete: true,
      completeness_reasons: [],
      visible_contract: visibleContract,
      published_contract: visibleContract,
      required_specs: docs.map((document) => ({
        path: document.path, sha256: document.sha256, byte_length: document.byte_length,
        content_included: true, next_step: null, text: document.body,
      })),
      authorized_readable_refs: docs.map((document) => ({
        kind: 'source', path: document.path, sha256: document.sha256, byte_length: document.byte_length,
      })),
      adopted_dependencies: [],
      ordinary_decisions: data.decisions.filter((decision) => decision.work_id === work.work_id && decision.state === 'open').slice(0, 3),
      next_step: work.next_step,
      guidance: observation.guidance,
      responsibility: observation.responsibility,
      runtime,
      observation,
      snapshot: {
        version: 1, consistency: 'repeatable_read', queried_at_unix_ms: data.now_unix_ms,
        project_revision: '47', source_snapshot_id: sourceSnapshot(projectId), coordinator_epoch: `epoch-${projectId}-47`,
      },
    });
  }

  function documentsList(projectId, query) {
    let documents = data.documents.filter((document) => document.project_id === projectId);
    if (query.work_id) documents = documentsForTask(task(query.work_id, projectId));
    if (query.workstream_id) {
      const taskIds = new Set(data.tasks.filter((item) => item.project_id === projectId
        && item.workstream_id === query.workstream_id).map((item) => item.work_id));
      documents = documents.filter((document) => document.used_task_ids.some((workId) => taskIds.has(workId)));
    }
    if (query.search) {
      const needle = query.search.toLocaleLowerCase();
      documents = documents.filter((document) => [document.name, document.summary, document.path]
        .some((value) => value.toLocaleLowerCase().includes(needle)));
    }
    const result = page(documents, query);
    return reply(projectId, query, {
      work_id: query.work_id ?? null,
      items: result.items.map((document, index) => ({
        document_position: result.offset + index + 1,
        path: document.path,
        name: document.name,
        summary: document.summary,
        summary_kind: 'source_excerpt',
        sha256: document.sha256,
        byte_length: document.byte_length,
        source_kind: 'registered_source',
        readable: true,
        status: 'available',
        reason: null,
        ...(query.work_id && document.content_work_id ? { content_work_id: document.content_work_id } : {}),
      })),
      total: result.total,
      next_cursor: result.next_cursor,
      truncated: result.truncated,
      read_only: true,
      execution_authorized: false,
      scope: query.work_id ? 'work' : 'authorized_workstreams',
      content_read_limit_bytes: 262144,
      as_of: asOf(projectId),
    });
  }

  function sourceContent(projectId, query) {
    const document = data.documents.find((item) => item.project_id === projectId && item.path === query.source_path);
    if (!document || query.expected_sha256 && query.expected_sha256 !== document.sha256) {
      throw new DemoReadError('Unavailable', 'source content is unavailable at the supplied digest', 503);
    }
    if (query.work_id && !documentsForTask(task(query.work_id, projectId)).includes(document)) {
      throw new DemoReadError('Forbidden', 'access denied', 403);
    }
    return reply(projectId, query, {
      path: document.path,
      sha256: document.sha256,
      byte_length: document.byte_length,
      text: document.body,
      kind: 'source',
      snapshot: 'active',
      history_bypass: false,
      path_bypass: false,
      url_bypass: false,
      next_step: null,
    });
  }

  function sourcesInspect(projectId, query) {
    let documents = data.documents.filter((document) => document.project_id === projectId);
    if (query.work_id) documents = documentsForTask(task(query.work_id, projectId));
    if (query.workstream_id) {
      const taskIds = new Set(data.tasks.filter((item) => item.project_id === projectId
        && item.workstream_id === query.workstream_id).map((item) => item.work_id));
      documents = documents.filter((document) => document.used_task_ids.some((workId) => taskIds.has(workId)));
    }
    const result = page(documents, query);
    return reply(projectId, query, {
      work_id: query.work_id ?? null,
      items: result.items.map((document, index) => ({
        source_position: result.offset + index + 1,
        path: document.path,
        expected_sha256: document.sha256,
        current_sha256: document.sha256,
        checked_at_unix_ms: data.now_unix_ms,
        status: 'consistent',
        reason: null,
        missing: { path: null, expected_sha256: null, current_sha256: null },
      })),
      total: result.total,
      next_cursor: result.next_cursor,
      truncated: result.truncated,
      execution_authorized: false,
      as_of: asOf(projectId),
      collection: {
        required_total: result.total,
        complete: true,
        completeness: 'complete',
        scope: query.work_id ? 'work' : 'authorized_workstreams',
        verification_basis: 'active_source_snapshot',
        recovery_scope: 'page',
        checked_at_unix_ms: data.now_unix_ms,
        reasons: [],
        recovery: [],
      },
    });
  }

  function decisionsList(projectId, query) {
    const state = query.decision_state ?? 'open';
    let decisions = data.decisions.filter((decision) => decision.project_id === projectId
      && (!query.work_id || decision.work_id === query.work_id)
      && (!query.workstream_id || decision.workstream_id === query.workstream_id)
      && (!query.decision_id || decision.decision_id === query.decision_id)
      && (state === 'all' || decision.state === state));
    decisions = decisions.sort((left, right) => right.decision_id.localeCompare(left.decision_id));
    const result = page(decisions, query);
    return reply(projectId, query, {
      work_id: query.work_id ?? null,
      decision_id: query.decision_id ?? null,
      decision_state: state,
      items: result.items.map((decision) => {
        const work = taskById.get(decision.work_id);
        const requester = actor(decision.requester_person_id);
        const responders = decision.allowed_responder_person_ids.map((personId, index) =>
          person(personId, decision.allowed_responder_display_names[index]));
        return {
          decision_id: decision.decision_id,
          work_id: decision.work_id,
          workstream_id: decision.workstream_id,
          session_id: `session-${decision.work_id.toLowerCase()}`,
          request_key: decision.request_key,
          work_title: work.title,
          question: decision.question,
          impact: decision.impact,
          decision_version: decision.decision_version,
          contract_hash: contractHash(decision.work_id),
          baseline_matches_current: decision.baseline_matches_current,
          requester,
          source_snapshot_id: sourceSnapshot(projectId),
          approval_granted: false,
          created_at_unix_ms: decision.created_at_unix_ms,
          allowed_responders: responders,
          allowed_responder_person_ids: responders.map((responder) => responder.person_id),
          state: decision.state,
          reply: decision.reply,
          resolution: decision.resolution ? {
            ...actor(decision.resolution.person_id, decision.resolution.display_name),
            kind: decision.resolution.kind,
            session_id: `session-${decision.work_id.toLowerCase()}`,
            request_id: decision.request_key,
            at_unix_ms: decision.resolution.at_unix_ms,
          } : null,
        };
      }),
      total: result.total,
      next_cursor: result.next_cursor,
      truncated: result.truncated,
      execution_authorized: false,
      as_of: asOf(projectId),
    });
  }

  function reviewRoundSummary(work, round) {
    const noDecision = round.state === 'open';
    const state = round.state;
    const values = {
      state,
      bundle_hash: round.bundle_hash,
      contract_hash: contractHash(work.work_id),
      contract_matches_current: round.contract_matches_current,
      artifact_digest: artifactDigest(work, round),
      reviewer_role: noDecision ? null : 'independent_reviewer',
      role_basis: noDecision ? null : 'recorded_approval_basis',
      reviewer_actor_id: noDecision ? null : round.reviewer_actor_id,
      reviewer_person_id: noDecision ? null : round.reviewer_person_id,
      decision: noDecision ? null : state === 'approved' ? 'approve' : state === 'rejected' ? 'reject' : null,
      decided_at_unix_ms: noDecision ? null : round.decided_at_unix_ms,
      reason_first_line: noDecision ? null : round.reason_first_line,
    };
    return {
      round_id: round.round_id,
      round_index: round.round_index,
      readable: true,
      reason: null,
      ...values,
      missing: Object.fromEntries(Object.entries(values).map(([key, value]) => [key, value === null ? 'not_recorded' : null])),
    };
  }

  function reviewRounds(projectId, query) {
    const work = task(query.work_id, projectId);
    const rounds = work.delivery?.rounds ?? [];
    const result = page(rounds, query);
    return reply(projectId, query, {
      work_id: work.work_id,
      items: result.items.map((round, index) => ({
        ...reviewRoundSummary(work, round), round_position: result.offset + index + 1,
      })),
      total: result.total,
      next_cursor: result.next_cursor,
      truncated: result.truncated,
      execution_authorized: false,
      as_of: { ...asOf(projectId), rounds_revision: sha256(`rounds:${work.work_id}`).slice(0, 32) },
    });
  }

  function roundForArtifact(projectId, query) {
    const work = task(query.work_id, projectId);
    const round = work.delivery?.rounds.find((item) => [item.artifact_id, item.evidence_id, item.execution_id, item.claim_id]
      .includes(query.artifact_id ?? query.evidence_id ?? query.execution_id ?? query.claim_id));
    if (!round) throw new DemoReadError('Forbidden', 'access denied', 403);
    return { work, round };
  }

  function reviewInspect(projectId, query) {
    const work = task(query.work_id, projectId);
    const round = work.delivery?.rounds.find((item) => item.round_id === query.review_round_id);
    if (!round) throw new DemoReadError('Forbidden', 'access denied', 403);
    const noDecision = round.state === 'open';
    const digest = artifactDigest(work, round);
    return reply(projectId, query, {
      review: {
        work_id: work.work_id,
        round_id: round.round_id,
        round_index: round.round_index,
        state: round.state,
        author_actor_id: round.author_actor_id,
        author_client_id: `${work.owner_person_id}-client`,
        author_person_id: round.author_person_id,
        artifact_digest: digest,
        execution_result_digest: digest,
        bundle_hash: round.bundle_hash,
        contract_hash: contractHash(work.work_id),
        evidence_id: round.evidence_id,
        execution_id: round.execution_id,
        candidate_current: round.current,
        decisions: noDecision ? [] : [{
          decision: round.state === 'approved' ? 'approve' : 'reject',
          decision_id: `decision-${round.round_id}`,
          approval_basis: 'simulated_member_independent_review',
          human_approval: false,
          independence_kind: 'simulated_member_independent',
          team_independent_acceptance: false,
          reviewer_actor_id: round.reviewer_actor_id,
          reviewer_client_id: `${round.reviewer_person_id}-client`,
          reviewer_person_id: round.reviewer_person_id,
          reason: round.reason_first_line,
        }],
        evidence_provenance: {
          author_self_report: true,
          binding_status: 'exact',
          currentness: round.current ? 'current_contract' : 'historical_contract',
          review_contract_matches_current: round.contract_matches_current,
          trust_basis: 'caller_asserted',
          evidence_inspection: { available: true, reason: null,
            query: { protocol_version: 1, op: 'evidence.inspect', work_id: work.work_id, evidence_id: round.evidence_id } },
          execution_inspection: { available: true, reason: null,
            query: { protocol_version: 1, op: 'execution.inspect', work_id: work.work_id, execution_id: round.execution_id } },
          execution_consistency: { claimed_success: true, execution_success: true,
            fact_source: 'team_execution_record', independent_verification: false,
            reason: 'caller_claim_matches_execution', recorded_state: 'succeeded', status: 'consistent' },
        },
        submission: {
          approval_inferred: false,
          candidate_current: round.current,
          candidate_digest: sha256(`candidate:${round.round_id}`),
          delivery_required: true,
          inspectable: true,
          reason: 'inspectable_current_submission',
          repository_verification_inferred: false,
          artifacts: [{
            byte_length: String(Buffer.byteLength(artifactText(work, round))),
            manifest_artifact_id: null,
            primary_evidence: true,
            query: { protocol_version: 1, op: 'artifact.content', work_id: work.work_id,
              artifact_id: round.artifact_id, expected_sha256: digest },
            segment_query: { protocol_version: 1, op: 'artifact.segment', work_id: work.work_id,
              artifact_id: round.artifact_id, expected_sha256: digest },
          }],
          source_navigation: { available: false, awr_checkout_verified: false,
            reason: 'source_locator_not_configured', note: 'No source locator is disclosed.' },
          source_revision: null,
        },
      },
    });
  }

  function evidenceInspect(projectId, query) {
    const { work, round } = roundForArtifact(projectId, { ...query, evidence_id: query.evidence_id });
    const digest = artifactDigest(work, round);
    return reply(projectId, query, {
      evidence: {
        work_id: work.work_id,
        evidence_id: round.evidence_id,
        artifact_id: round.artifact_id,
        artifact_digest: digest,
        execution_result_digest: digest,
        contract_hash: contractHash(work.work_id),
        execution_id: round.execution_id,
        digest: round.bundle_hash,
        created_by: round.author_actor_id,
        trust_basis: 'caller_asserted',
        execution_consistency: { claimed_success: true, execution_success: true,
          fact_source: 'team_execution_record', independent_verification: false,
          reason: 'caller_claim_matches_execution', recorded_state: 'succeeded', status: 'consistent' },
      },
    });
  }

  function executionInspect(projectId, query) {
    const { work, round } = roundForArtifact(projectId, query);
    const execution = snapshot(projectId, { ...query, op: 'work.snapshot' }).data.observation.execution;
    if (!execution) throw new DemoReadError('Forbidden', 'access denied', 403);
    return reply(projectId, query, {
      ...execution,
      execution_id: round.execution_id,
      claim_id: round.claim_id,
      contract_matches_current: round.current,
    });
  }

  function claimInspect(projectId, query) {
    const work = task(query.work_id, projectId);
    const round = work.delivery?.rounds.find((item) => item.claim_id === query.claim_id);
    if (!round) throw new DemoReadError('Forbidden', 'access denied', 403);
    return reply(projectId, query, {
      claim_id: round.claim_id,
      current_fence: true,
      epoch_matches_current: true,
      execution_authorized: false,
      execution_eligibility_advisory: false,
      expires_at: null,
      fence: '1',
      lease_live: Boolean(work.agent_active),
      lease_version: '1',
      owned_by_client: false,
      session_id: `session-${work.work_id.toLowerCase()}`,
      state: 'active',
    });
  }

  function completionInspect(projectId, query) {
    const work = task(query.work_id, projectId);
    const round = latestRound(work);
    if (!round || statusOf(work) !== 'verified') {
      return reply(projectId, query, { completion: null, runtime_state: runtimeState(work), selected_completion_id: null });
    }
    const selected = `completion-${round.round_id}`;
    return reply(projectId, query, {
      selected_completion_id: selected,
      runtime_state: 'completed',
      completion: {
        receipt_id: selected,
        contract_hash: contractHash(work.work_id),
        evidence_id: round.evidence_id,
        execution_id: round.execution_id,
        author_actor_id: round.author_actor_id,
        executor_actor_id: round.author_actor_id,
        final_submitter_actor_id: round.author_actor_id,
        submitted_by_person_id: work.owner_person_id,
        approved_by_person_id: round.reviewer_person_id,
        reviewer_actor_id: round.reviewer_actor_id,
        reviewer_client_id: `${round.reviewer_person_id}-client`,
        independence_kind: 'simulated_member_independent',
        policy: 'caller_managed_execution_and_simulated_member_review',
        provider_private_session: null,
        owner_person_id: work.owner_person_id,
        pr_delivery_id: null,
        review_round_id: round.round_id,
        review_decision_id: `decision-${round.round_id}`,
        approved_by: {
          approval_basis: 'simulated_member_independent_review',
          approved_by: round.reviewer_actor_id,
          approved_by_person_id: round.reviewer_person_id,
          author_actor_id: round.author_actor_id,
          author_self_report: true,
          execution_basis: 'caller_asserted_workspace_settled',
          execution_success: true,
          executor_actor_id: round.author_actor_id,
          final_submitter_actor_id: round.author_actor_id,
          github_merged_does_not_complete: true,
          human_approval: false,
          independence_kind: 'simulated_member_independent',
          review_round_id: round.round_id,
          review_decision_id: `decision-${round.round_id}`,
          team_independent_acceptance: false,
        },
      },
    });
  }

  function deliveryInspect(projectId, query) {
    const work = task(query.work_id, projectId);
    const round = latestRound(work);
    const selected = statusOf(work) === 'verified' && round ? `completion-${round.round_id}` : null;
    const dependencies = work.dependencies.map((dependency) => dependencyShape(projectId, work, dependency, false));
    return reply(projectId, query, {
      delivery: {
        work_id: work.work_id,
        awr_acceptance: {
          complete: statusOf(work) === 'verified',
          runtime_state: statusOf(work) === 'verified' ? 'completed' : runtimeState(work),
          selected_completion_id: selected,
        },
        cannot_skip_acceptance_via: ['pr_url_alone', 'green_ci', 'admin_role', 'already_merged'],
        github: { approved: false, merged: false, submitted: false },
        neutral_observation: null,
        pr: null,
        webhook_auto_sync: false,
        ...(work.delivery ? { dependencies } : {}),
      },
    });
  }

  function dependencyShape(projectId, consumer, dependency, collection = true) {
    const provider = taskById.get(dependency.provider_work_id);
    const providerRound = latestRound(provider);
    const requiredVersion = {
      receipt_id: null,
      contract_hash: contractHash(provider.work_id),
      source_snapshot_id: sourceSnapshot(projectId),
    };
    const availableVersion = statusOf(provider) === 'verified' && providerRound ? {
      receipt_id: `completion-${providerRound.round_id}`,
      contract_hash: contractHash(provider.work_id),
      source_snapshot_id: sourceSnapshot(projectId),
    } : null;
    const consumed = dependency.satisfied && consumer.started ? availableVersion : null;
    const versionFields = ['required_version', 'available_version', 'adopted_version', 'consumed_version'];
    const versions = {
      required_version: requiredVersion,
      available_version: availableVersion,
      adopted_version: consumed,
      consumed_version: consumed,
    };
    return {
      work_id: consumer.work_id,
      provider_work_id: provider.work_id,
      provider_title: provider.title,
      version_policy: dependency.version_policy,
      review_assurance: null,
      ...versions,
      adoption_valid: null,
      validity: dependency.validity,
      reason: dependency.reason,
      missing: {
        provider_title: null,
        ...Object.fromEntries(versionFields.map((field) => [field, versions[field] === null ? 'not_recorded' : null])),
      },
      ...(collection ? {
        work_title: consumer.title,
        dependency_position: dependency.position,
      } : {}),
    };
  }

  function dependenciesList(projectId, query) {
    const tasks = data.tasks.filter((item) => item.project_id === projectId
      && (!query.work_id || item.work_id === query.work_id)
      && (!query.workstream_id || item.workstream_id === query.workstream_id));
    const dependencies = tasks.flatMap((item) => item.dependencies.map((dependency) => ({ item, dependency })));
    const result = page(dependencies, query);
    const response = reply(projectId, query, {
      work_id: query.work_id ?? null,
      items: result.items.map(({ item, dependency }, index) => ({
        ...dependencyShape(projectId, item, dependency), dependency_position: result.offset + index + 1,
      })),
      total: result.total,
      next_cursor: result.next_cursor,
      truncated: result.truncated,
      count_scope: 'authorized_matches',
      count_limited: true,
      hidden_count: null,
      execution_authorized: false,
      as_of: asOf(projectId),
    });
    delete response.selection_basis;
    return response;
  }

  function handoffsList(projectId, query) {
    const state = query.handoff_state ?? 'pending';
    const handoffs = data.handoffs.filter((handoff) => handoff.project_id === projectId
      && (!query.work_id || handoff.work_id === query.work_id)
      && (!query.workstream_id || handoff.workstream_id === query.workstream_id)
      && (!query.recipient_person_id || handoff.to_person_id === query.recipient_person_id)
      && (state === 'all' || handoff.group === state))
      .sort((left, right) => right.created_at_unix_ms - left.created_at_unix_ms
        || left.handoff_id.localeCompare(right.handoff_id));
    const result = page(handoffs, query);
    return reply(projectId, query, {
      work_id: query.work_id ?? null,
      recipient_person_id: query.recipient_person_id ?? null,
      handoff_state: state,
      items: result.items.map((handoff, index) => ({
        handoff_id: handoff.handoff_id,
        handoff_position: result.offset + index + 1,
        work_id: handoff.work_id,
        work_title: taskById.get(handoff.work_id).title,
        kind: handoff.kind,
        status: handoff.status,
        pending: handoff.pending,
        expired: handoff.expired,
        group: handoff.group,
        effective_status: handoff.effective_status,
        version: handoff.version,
        from_person_id: handoff.from_person_id,
        to_person_id: handoff.to_person_id,
        from_display_name: handoff.from_display_name,
        to_display_name: handoff.to_display_name,
        created_at_unix_ms: handoff.created_at_unix_ms,
        updated_at_unix_ms: handoff.updated_at_unix_ms,
        expires_at_unix_ms: handoff.expires_at_unix_ms,
      })),
      total: result.total,
      next_cursor: result.next_cursor,
      truncated: result.truncated,
      count_scope: 'authorized_matches',
      count_limited: true,
      hidden_count: null,
      execution_authorized: false,
      as_of: { ...asOf(projectId), handoffs_revision: sha256(`handoffs:${projectId}`).slice(0, 32) },
    });
  }

  function planningCandidates(projectId, query) {
    const candidates = data.planning_candidates.filter((candidate) => candidate.project_id === projectId
      && (!query.workstream_id || candidate.workstream_ids.includes(query.workstream_id)));
    const result = page(candidates, query);
    const missing = (reason) => ({ value: null, reason });
    return reply(projectId, query, {
      items: result.items.map((candidate) => ({
        candidate_id: candidate.candidate_id,
        state: candidate.state,
        draft_revision: candidate.draft_revision,
        author: { ...person(candidate.author_person_id, candidate.author_display_name),
          actor_id: `${candidate.author_person_id}-agent`, client_id: `${candidate.author_person_id}-client` },
        updated_at_unix_ms: candidate.updated_at_unix_ms,
        baseline: candidate.baseline,
        current_diff: {
          counts: { added: candidate.items.length, removed: 0, changed: 0, unchanged: 0 },
          items: candidate.items.map((item) => ({
            work_id: item.work_id, title: item.title, kind: 'added',
            fields: [{ field: 'acceptance', before: { value: null, reason: 'not_present' },
              after: { value: [item.kind], reason: null } }],
          })),
        },
        approval: missing('not_approved'),
        publication: missing('not_published'),
        activation: missing('not_activated'),
        rejection: missing('not_rejected'),
      })),
      total: result.total,
      next_cursor: result.next_cursor,
      truncated: result.truncated,
      execution_authorized: false,
      as_of: asOf(projectId),
    });
  }

  function auditClientId(item) {
    const actor = /^(M-\d{3})-agent$/.exec(item.actor_id);
    if (!actor) return null;
    const member = personById.get(actor[1]);
    const agent = member?.agents.find((candidate) => item.actor_display_name?.includes(candidate.client_name))
      ?? member?.agents[0];
    const client = agent?.client_name === 'Codex' ? 'codex'
      : agent?.client_name.startsWith('Claude') ? 'claude' : null;
    return client ? `${actor[1]}-${client}` : null;
  }

  // AWR-PPD-055 disclosure on the demo facts: each fixture event records its attribution
  // as the phrase the prototype writes. Decompose it into the audit's own fields — the
  // person's display name, the client's display name, and whether the operation is
  // attributed to that client (client_id) or performed by the person through it.
  //   'Claude 桌面（林悦）'            → actor 林悦, client Claude 桌面, client_id set
  //   '周宁（通过 Claude 桌面）'        → actor 周宁, client Claude 桌面, client_id null
  //   '陈宇的 Agent' / '许宁'          → the recorded attribution, no client named
  function auditIdentity(item) {
    const phrase = typeof item.actor_display_name === 'string' ? item.actor_display_name : '';
    let match = /^(.+?)（通过 (.+?)）$/.exec(phrase);
    if (match) return { person: match[1], client: match[2], attributed: false };
    match = /^(.+?)（(.+?)）$/.exec(phrase);
    if (match) return { person: match[2], client: match[1], attributed: true };
    return { person: phrase, client: null, attributed: false };
  }

  function audit(projectId, query) {
    const kind = query.op === 'audit.development' ? 'development' : 'requests';
    const items = data.activity.filter((item) => item.project_id === projectId
      && (!query.work_id || item.work_id === query.work_id)
      && (!query.member_actor_id || item.actor_id === query.member_actor_id));
    const result = page(items, query, 50);
    return reply(projectId, query, {
      items: result.items.map((item) => {
        const identity = auditIdentity(item);
        const actor = /^M-\d{3}$|^(M-\d{3})-agent$/.exec(item.actor_id);
        return {
          id: item.id,
          actor_id: item.actor_id,
          client_id: identity.attributed ? auditClientId(item) : null,
          actor: {
            person_id: actor ? (actor[1] ?? actor[0]) : null,
            display_name: identity.person || null,
          },
          client: { display_name: identity.client },
          action: kind === 'development' ? item.action : 'query',
          work_id: item.work_id,
          result: kind === 'development' ? item.result : 'succeeded',
          created_at_unix_ms: item.created_at_unix_ms,
          credential_id: item.credential_id,
        };
      }),
      next_cursor: result.next_cursor,
      development_source: query.op,
      chat_text_collected: false,
      tool_io_collected: false,
      request_retention: '30d',
      scope: 'project',
      unknown_means: 'not_observed',
      execution_authorized: false,
      as_of: asOf(projectId),
    });
  }

  function artifactContent(projectId, query) {
    const { work, round } = roundForArtifact(projectId, query);
    const text = artifactText(work, round);
    const digest = artifactDigest(work, round);
    if (query.expected_sha256 && query.expected_sha256 !== digest) {
      throw new DemoReadError('Unavailable', 'artifact content is unavailable at the supplied digest', 503);
    }
    if (query.op === 'artifact.segment') {
      let start = 0;
      if (query.cursor) {
        const match = /^artifact-offset-([1-9][0-9]*)$/.exec(query.cursor);
        if (!match) throw new DemoReadError('CursorExpired', 'cursor expired', 409);
        start = Number(match[1]);
      }
      const size = Math.min(query.max_context_bytes ?? 2048, 4096);
      const bytes = Buffer.from(text);
      const segment = bytes.subarray(start, Math.min(bytes.length, start + size));
      const end = start + segment.length;
      const done = end >= bytes.length;
      return reply(projectId, query, {
        artifact_id: round.artifact_id,
        work_id: work.work_id,
        kind: 'artifact_segment',
        byte_length: bytes.length,
        content_base64: segment.toString('base64'),
        encoding: 'base64',
        range: { start, end_exclusive: end },
        sha256: digest,
        segment_sha256: sha256(segment),
        next_cursor: done ? null : `artifact-offset-${end}`,
        next_query: done ? null : { protocol_version: 1, op: 'artifact.segment', work_id: work.work_id,
          artifact_id: round.artifact_id, expected_sha256: digest, cursor: `artifact-offset-${end}`, max_context_bytes: size },
        end_of_artifact: done,
        execution_authorized: false,
        adopted: null,
        disclosure_sha256: null,
        export_id: null,
        export_version: null,
        path_bypass: false,
        upstream_source_access: false,
        url_bypass: false,
        consumption: 'Concatenate contiguous ranges from byte 0, then verify byte_length and sha256.',
      });
    }
    return reply(projectId, query, {
      artifact_id: round.artifact_id,
      work_id: work.work_id,
      kind: 'artifact',
      byte_length: Buffer.byteLength(text),
      content_base64: null,
      text,
      sha256: digest,
      media_type: 'text/markdown; charset=utf-8',
      history_bypass: false,
      path_bypass: false,
      url_bypass: false,
      next_step: null,
    });
  }

  function updatesList(projectId, query) {
    const updates = data.updates.filter((update) => update.project_id === projectId)
      .sort((left, right) => right.starts_on.localeCompare(left.starts_on)
        || right.ends_on.localeCompare(left.ends_on) || right.period_key.localeCompare(left.period_key)
        || right.update_id.localeCompare(left.update_id));
    const result = page(updates, query);
    return reply(projectId, query, {
      project_id: projectId,
      items: result.items.map((update) => updateRecord(update)),
      total: result.total,
      next_cursor: result.next_cursor,
      truncated: result.truncated,
      execution_authorized: false,
      as_of: asOf(projectId),
    });
  }

  function updateRecord(update) {
    return {
      update_id: update.update_id,
      period_kind: update.period_kind,
      period_key: update.period_key,
      starts_on: update.starts_on,
      ends_on: update.ends_on,
      status: update.status,
      summary: update.summary,
      author: person(update.author_person_id),
      recorded_at_unix_ms: update.recorded_at_unix_ms,
      version: update.version,
      supersedes_update_id: update.supersedes_update_id,
      fact_snapshot_ref: update.fact_snapshot.fact_snapshot_ref,
      current: update.current,
    };
  }

  function goalRecord(goal) {
    const tasks = goal.task_ids.map((workId) => taskById.get(workId)).filter(Boolean);
    return {
      goal_id: goal.goal_id,
      project_id: goal.project_id,
      title: goal.title,
      status: goal.status,
      health: goal.health,
      summary: goal.summary,
      trend: goal.trend,
      department: data.organization.departments.find((department) => department.id === goal.department_id) ?? null,
      criteria: goal.criteria,
      tasks: tasks.map((work) => ({ work_id: work.work_id, title: work.title, status: runtimeState(work) })),
      hidden_task_count: 0,
      uncovered: tasks.length === 0,
      orphan_tasks: [],
    };
  }

  function lessonRecord(lesson) {
    return {
      lesson_id: lesson.lesson_id,
      project_id: lesson.project_id,
      adopted_project_ids: lesson.adopted_project_ids,
      title: lesson.title,
      stage: lesson.stage,
      state: lesson.state,
      quote: lesson.quote,
      author: person(lesson.author_person_id),
      source_title: lesson.source_title,
      source_task_id: lesson.source_task_id,
      applicable_when: lesson.applicable_when,
      not_applicable_when: lesson.not_applicable_when,
      result: lesson.result,
      usage_count: lesson.usage_records.length,
      passed_count: lesson.usage_records.filter((record) => record.outcome === 'passed').length,
      not_applicable_count: lesson.usage_records.filter((record) => record.outcome === 'not_applicable').length,
      tasks: lesson.usage_records.map((record) => record.task_id),
      usage_records: lesson.usage_records,
      recheck_required: lesson.recheck_required,
      recheck_reason: lesson.recheck_reason,
      disposition: lesson.disposition,
      pending_confirmation: lesson.pending_id !== null && lesson.state === 'active' && lesson.stage === 'candidate',
    };
  }

  function read(projectId, query) {
    project(projectId);
    guard(projectId, query);
    switch (query.op) {
      case 'capabilities': return {
        protocol: 'awr-team-workstream', protocol_version: 1,
        business_role_presets: clone(DEMO_BUSINESS_ROLE_PRESETS),
        identity: { ...viewerIdentity(), can_manage_members: false, can_read_project_audit: true,
          membership_action_ceiling: ['work.read'] },
        queries: ['capabilities', 'workstreams.list', 'work.list', 'work.graph', 'work.search', 'work.next', 'work.inbox',
          'work.prepare', 'work.snapshot', 'work.observe', 'work.admission', 'work.recovery', 'events.list',
          'sources.inspect', 'source.content', 'documents.list', 'decisions.list', 'review.rounds', 'review.inspect',
          'handoff.list', 'handoff.inspect', 'dependencies.list', 'planning.candidates', 'planning.outcome',
          'evidence.inspect', 'execution.inspect', 'claim.inspect', 'completion.inspect', 'delivery.inspect',
          'delivery.neutral.inspect', 'delivery.submission.describe', 'delivery.integration.inspect',
          'delivery.source.status', 'delivery.exports', 'artifact.content', 'artifact.segment',
          'audit.development', 'audit.requests', 'task.assignees', 'session.inspect', 'command.inspect',
          'usage.summary',
          ...DEMO_FUTURE_READ_OPS],
        controlled_content_reads: true,
        artifact_content: true,
        source_content: true,
        execution_authorized: false,
      };
      case 'workstreams.list': return workstreams(projectId, query);
      case 'work.list':
      case 'work.search': return workList(projectId, query);
      case 'work.graph': return workGraph(projectId, query);
      case 'work.next': return workNext(projectId, query);
      case 'work.inbox': return workInbox(projectId, query);
      case 'work.snapshot': return snapshot(projectId, query);
      case 'work.prepare': {
        const result = snapshot(projectId, query);
        delete result.data.observation;
        delete result.data.runtime;
        delete result.data.snapshot;
        return result;
      }
      case 'work.observe': {
        const result = snapshot(projectId, query);
        return reply(projectId, query, result.data.observation);
      }
      case 'work.admission': return reply(projectId, query, { governed: false, basis: 'the snapshot declares no governance' });
      case 'work.recovery': return reply(projectId, query, {
        work_id: query.work_id,
        automatic_resume: false,
        candidates: task(query.work_id, projectId).agent_name ? [{
          session_id: `session-${query.work_id.toLowerCase()}`,
          checkpoint_id: `checkpoint-${query.work_id.toLowerCase()}`,
          contract_matches_current: true,
        }] : [],
      });
      case 'documents.list': return documentsList(projectId, query);
      case 'source.content': return sourceContent(projectId, query);
      case 'sources.inspect': return sourcesInspect(projectId, query);
      case 'decisions.list': return decisionsList(projectId, query);
      case 'usage.summary': return usageSummary(projectId, query);
      case 'review.rounds': return reviewRounds(projectId, query);
      case 'review.inspect': return reviewInspect(projectId, query);
      case 'evidence.inspect': return evidenceInspect(projectId, query);
      case 'execution.inspect': return executionInspect(projectId, query);
      case 'claim.inspect': return claimInspect(projectId, query);
      case 'completion.inspect': return completionInspect(projectId, query);
      case 'delivery.inspect': return deliveryInspect(projectId, query);
      case 'delivery.neutral.inspect': {
        const work = task(query.work_id, projectId);
        return reply(projectId, query, { work_id: work.work_id, acceptance_ready: statusOf(work) === 'verified',
          candidate: null, connectors: [], connectors_truncated: false, facts: [], facts_truncated: false,
          history: [], execution_authorized: false });
      }
      case 'delivery.neutral.outcome': {
        const work = task(query.work_id, projectId);
        return reply(projectId, query, { work_id: work.work_id, outcome: 'unknown', receipt: null,
          execution_authorized: false });
      }
      case 'delivery.submission.describe': {
        const work = task(query.work_id, projectId);
        return reply(projectId, query, { work_id: work.work_id, acceptance_ready: statusOf(work) === 'verified',
          candidate: null, execution_authorized: false });
      }
      case 'delivery.integration.inspect': {
        const work = task(query.work_id, projectId);
        const integrationId = work.integrated && latestRound(work) ? `integration-${latestRound(work).round_id}` : null;
        if (!integrationId || integrationId !== query.request_id) throw new DemoReadError('Forbidden', 'access denied', 403);
        const observedAt = work.verified_at_unix_ms ?? data.now_unix_ms;
        return reply(projectId, query, {
          integration_id: integrationId,
          work_id: work.work_id,
          state: 'confirmed',
          current_binding: true,
          confirmation_current: true,
          candidate: null,
          confirmation: {
            envelope: { protocol: 'awr-delivery', protocol_version: 1, record: {
              kind: 'integration_observation',
              data: { outcome: 'applied', provenance: { observed_at_unix_ms: observedAt, recorded_at_unix_ms: observedAt } },
            } },
          },
          dispatch_receipt: { data: { state: 'confirmed' } },
        });
      }
      case 'delivery.source.status': {
        const work = task(query.work_id, projectId);
        return reply(projectId, query, {
          work_id: work.work_id,
          source_observation: 'observed',
          source_synchronized: true,
          cursor_current: true,
          projection_current: true,
          history: [],
          history_truncated: false,
          metadata_revision: '1',
          pending_publication_id: null,
          source_fingerprint: `sha256:${sha256(`source:${work.work_id}`)}`,
          confirmed_fingerprint: `sha256:${sha256(`source:${work.work_id}`)}`,
          acceptance_ready: statusOf(work) === 'verified',
          execution_authorized: false,
        });
      }
      case 'delivery.exports': return reply(projectId, query, {
        work_id: query.work_id, items: [], total: 0, next_cursor: null, truncated: false,
        execution_authorized: false, as_of: asOf(projectId),
      });
      case 'dependencies.list': return dependenciesList(projectId, query);
      case 'handoff.list': return handoffsList(projectId, query);
      case 'handoff.inspect': {
        const handoff = data.handoffs.find((item) => item.project_id === projectId && item.handoff_id === query.handoff_id);
        if (!handoff) throw new DemoReadError('Forbidden', 'access denied', 403);
        return reply(projectId, query, { handoff, work_id: handoff.work_id, execution_authorized: false });
      }
      case 'planning.candidates': return planningCandidates(projectId, query);
      case 'planning.outcome':
      case 'command.inspect': return reply(projectId, query, { outcome: 'unknown', receipt: null, execution_authorized: false });
      case 'artifact.content':
      case 'artifact.segment': return artifactContent(projectId, query);
      case 'audit.development':
      case 'audit.requests': return audit(projectId, query);
      case 'task.assignees': return reply(projectId, query, {
        work_id: query.work_id,
        items: data.people.filter((member) => project(projectId).member_person_ids.includes(member.person_id))
          .map((member) => ({ person_id: member.person_id, display_name: member.display_name, role: member.role })),
        total: project(projectId).member_person_ids.length, next_cursor: null, truncated: false, execution_authorized: false,
      });
      case 'events.list': return audit(projectId, { ...query, op: 'audit.development' });
      case 'session.inspect': return reply(projectId, query, {
        session: { id: query.session_id, state: 'active', checkpoint: null },
        execution_authorized: false,
      });
      case 'updates.list': return updatesList(projectId, query);
      case 'update.inspect': {
        const update = updateById.get(query.update_id);
        if (!update || update.project_id !== projectId) throw new DemoReadError('Forbidden', 'access denied', 403);
        return reply(projectId, query, { project_id: projectId, update: updateRecord(update),
          execution_authorized: false, as_of: asOf(projectId) });
      }
      case 'goals.list': {
        const goals = data.goals.filter((goal) => goal.project_id === projectId);
        const result = page(goals, query);
        return reply(projectId, query, { items: result.items.map(goalRecord), total: result.total,
          next_cursor: result.next_cursor, truncated: result.truncated, execution_authorized: false, as_of: asOf(projectId) });
      }
      case 'goal.inspect': {
        const goal = goalById.get(query.goal_id);
        if (!goal || goal.project_id !== projectId) throw new DemoReadError('Forbidden', 'access denied', 403);
        return reply(projectId, query, { goal: goalRecord(goal), execution_authorized: false, as_of: asOf(projectId) });
      }
      case 'lessons.list': {
        const lessons = data.lessons.filter((lesson) => lesson.project_id === projectId);
        const result = page(lessons, query);
        return reply(projectId, query, { items: result.items.map(lessonRecord), total: result.total,
          next_cursor: result.next_cursor, truncated: result.truncated, execution_authorized: false, as_of: asOf(projectId) });
      }
      case 'lesson.inspect': {
        const lesson = lessonById.get(query.lesson_id);
        if (!lesson || lesson.project_id !== projectId) throw new DemoReadError('Forbidden', 'access denied', 403);
        return reply(projectId, query, { lesson: lessonRecord(lesson), execution_authorized: false, as_of: asOf(projectId) });
      }
      default: throw new DemoReadError('Unsupported', 'read operation is not available', 501);
    }
  }

  function upstream(projectId, queryBody) {
    try {
      return { status: 200, json: read(projectId, queryBody), setCookie: [] };
    } catch (error) {
      if (error instanceof DemoReadError) {
        return { status: error.status, json: { code: error.code, message: error.message }, setCookie: [] };
      }
      throw error;
    }
  }

  function directory(projectId) {
    const selected = project(projectId);
    if (variant === 'empty') {
      return { status: 200, json: { data: {
        project_id: projectId, configured: false, version: '0', updated_at_ms: null,
        plan: { organization: null, departments: [], project_department_ids: [], members: [] }, members: [],
      } }, setCookie: [] };
    }
    if (variant === 'restricted' && projectId === 'P-001') {
      return { status: 403, json: { code: 'Forbidden', message: 'access denied' }, setCookie: [] };
    }
    const members = data.people.filter((member) => selected.member_person_ids.includes(member.person_id));
    // Canonical records the viewer's all-projects scope, so its directory carries the whole
    // visible hierarchy (every visible project's group and parent department) instead of one
    // project's slice; every other variant keeps the single-project slice byte for byte.
    const scopeProjectList = variant === 'canonical' ? data.projects : [selected];
    const departmentIds = new Set([...scopeProjectList.flatMap((project) => [project.group_id,
      project.parent_department_id]), ...members.map((member) => member.group_id)]);
    const departments = data.organization.departments.filter((department) => departmentIds.has(department.id));
    return { status: 200, json: { data: {
      project_id: projectId,
      configured: true,
      version: '1',
      updated_at_ms: data.now_unix_ms,
      plan: {
        organization: { id: data.organization.id, name: data.organization.name },
        departments: departments.map((department) => ({ id: department.id, name: department.name, parent_id: department.parent_id })),
        project_department_ids: [selected.group_id],
        members: members.map((member) => ({ person_id: member.person_id, department_ids: [member.group_id] })),
      },
      members: members.map((member) => ({
        member_ref: { project_id: projectId, person_id: member.person_id },
        available: true,
        display_name: member.display_name,
        department_ids: [member.group_id],
        ...(variant === 'canonical' ? { duty: member.role } : {}),
      })),
    } }, setCookie: [] };
  }

  function connections(projectId, body) {
    const selected = projectId === null && variant === 'canonical' ? null : project(projectId);
    const items = data.people.filter((member) => selected === null || selected.member_person_ids.includes(member.person_id))
      .flatMap((member) => member.agents.map((agent) => ({
        connection_id: agent.connection_id,
        kind: 'agent',
        client_name: `${member.display_name} · ${agent.client_name}`,
        state: revokedConnections.has(agent.connection_id) ? 'revoked' : agent.state,
        can_revoke: agent.can_revoke,
        revocation_scope: agent.revocation_scope,
        created_at_ms: agent.created_at_ms,
        expires_at_ms: agent.expires_at_ms,
        last_seen_ms: agent.last_seen_ms,
        revoked_at_ms: revokedConnections.has(agent.connection_id) ? data.now_unix_ms : null,
        is_current: agent.is_current,
      })));
    return { status: 200, json: { data: { items, next_cursor: null } }, setCookie: [] };
  }

  function revokeConnection(projectId, body) {
    const selected = project(projectId);
    const found = data.people.filter((member) => selected.member_person_ids.includes(member.person_id))
      .flatMap((member) => member.agents).find((agent) => agent.connection_id === body.connection_id);
    if (!found) return { status: 403, json: { code: 'Forbidden', message: 'access denied' }, setCookie: [] };
    revokedConnections.add(body.connection_id);
    return { status: 200, json: { data: {
      connection_id: body.connection_id, state: 'revoked', ends_current: false,
    } }, setCookie: [] };
  }

  function usageSummary(projectId, query = {}) {
    const proj = project(projectId);
    const units = new Map(data.organization.departments.map((department) => [department.id, department]));
    const group = units.get(proj.group_id);
    const tasks = data.tasks.filter((item) => item.project_id === proj.project_id
      && (!query.work_id || item.work_id === query.work_id)
      && (!query.workstream_id || item.workstream_id === query.workstream_id));
    if (query.work_id && tasks.length === 0) throw new DemoReadError('Forbidden', 'access denied', 403);
    const notCollectedRollup = {
      session_count: 0,
      active_session_count: 0,
      duration: { status: 'not_collected', summed_ms: null, wall_clock_ms: null, sessions_with_duration: 0, sessions_without_duration: 0 },
      tokens: { status: 'not_collected', input_tokens: null, output_tokens: null, cached_input_tokens: null, sessions_with_usage: 0, sessions_without_usage: 0 },
      cost: { status: 'not_collected', amount_micros: null, currency: null },
    };
    const collectedGroup = proj.project_id === 'P-001' && group;
    const byTask = [...tasks].sort((a, b) => a.work_id.localeCompare(b.work_id)).map((t) => ({
      work_id: t.work_id,
      workstream_id: t.workstream_id,
      ...structuredClone(notCollectedRollup),
    }));
    const projectMembers = data.people
      .filter((p) => proj.member_person_ids.includes(p.person_id))
      .sort((a, b) => a.person_id.localeCompare(b.person_id));
    const collectedMembers = collectedGroup && projectMembers.length > 0 ? [projectMembers[0]] : [];
    const missingMembers = collectedGroup && projectMembers.length > 0 ? projectMembers.slice(1) : projectMembers;
    const toMemberCov = (m) => ({
      member_id: m.person_id, member_kind: 'person', person_id: m.person_id,
      display_name: m.display_name, group_ids: m.group_id ? [m.group_id] : [],
    });
    const projectRollup = collectedGroup ? {
      session_count: 1,
      active_session_count: 0,
      duration: { status: 'collected', summed_ms: 1_800_000, wall_clock_ms: 1_800_000, sessions_with_duration: 1, sessions_without_duration: 0 },
      tokens: { status: 'collected', input_tokens: 6_400, output_tokens: 1_200, cached_input_tokens: 800, sessions_with_usage: 1, sessions_without_usage: 0 },
      cost: { status: 'not_collected', amount_micros: null, currency: null },
    } : structuredClone(notCollectedRollup);
    const byMember = projectMembers.map((m, idx) => ({
      ...toMemberCov(m),
      actor_ids: [m.person_id],
      ...(collectedGroup && idx === 0 ? structuredClone(projectRollup) : structuredClone(notCollectedRollup)),
    }));
    const usageStatus = collectedGroup ? (missingMembers.length === 0 ? 'complete' : 'partial') : 'not_collected';
    return reply(projectId, query, {
      project_id: proj.project_id,
      scope: query.work_id ? 'work' : query.workstream_id ? 'workstream' : 'project',
      work_id: query.work_id ?? null,
      workstream_id: query.workstream_id ?? null,
      collection: {
        personal_ledger_collected: false,
        billing_collected: false,
        cost_status: 'not_collected',
        usage_scope: 'host_session',
        usage_provenance: 'caller_declared',
        duration_basis: 'recorded_session_events',
        is_historical_observation: true,
        is_not_estimated_remaining_time: true,
      },
      project: projectRollup,
      by_task: byTask,
      by_member: byMember,
      member_scope: 'project',
      member_ordering: 'identifier_asc',
      personal_comparison_ranking: false,
      coverage: {
        status: usageStatus,
        cost_status: 'not_collected',
        usage_status: usageStatus,
        duration_status: collectedGroup ? 'complete' : 'not_collected',
        hidden_work_count: 0,
        groups: {
          registered: Boolean(group),
          collected: collectedGroup ? [{ id: group.id, name: group.name }] : [],
          missing: !collectedGroup && group ? [{ id: group.id, name: group.name }] : [],
        },
        members: {
          scope: 'project',
          collected: collectedMembers.map(toMemberCov),
          missing: missingMembers.map(toMemberCov),
          hidden_member_count: 0,
        },
      },
      as_of: { at_unix_ms: data.now_unix_ms, source_snapshot_id: `snapshot-${proj.project_id}-47`, project_revision: '47' },
      execution_authorized: false,
    });
  }

  function portfolioOverview(query = {}) {
    const units = new Map(data.organization.departments.map((department) => [department.id, department]));
    const currentWeekStart = utcMondayWeek(data.now_unix_ms).startUnixMs;
    const fixtureWeekStart = utcMondayWeek(completeOrganization.now_unix_ms).startUnixMs;
    const items = data.projects.map((proj) => {
      if (variant === 'restricted' && proj.project_id === 'P-003') {
        return { project_id: proj.project_id, project_key: proj.key, read: { status: 'not_read', reason: 'no_read_authority' } };
      }
      if (variant === 'partial' && proj.project_id === 'P-001') {
        return { project_id: proj.project_id, project_key: proj.key, read: { status: 'not_read', reason: 'read_failed' } };
      }
      const tasks = data.tasks.filter((item) => item.project_id === proj.project_id);
      let verified = 0;
      let deliveredUnverified = 0;
      let inProgress = 0;
      let notStarted = 0;
      for (const t of tasks) {
        const st = statusOf(t);
        if (st === 'verified') verified += 1;
        else if (st === 'claimed') deliveredUnverified += 1;
        else if (st === 'ready' || st === 'planned') notStarted += 1;
        else inProgress += 1;
      }
      const openDecisions = data.decisions.filter((d) => d.project_id === proj.project_id && d.state === 'open');
      const openReviews = tasks.flatMap((t) => (t.delivery?.rounds || []).filter((r) => r.state === 'open'));
      const depWaits = tasks.filter((t) => t.wait?.kind === 'dep');
      const openTimes = [
        ...openDecisions.map((d) => d.created_at_unix_ms),
        ...openReviews.map((r) => r.opened_at_unix_ms),
        ...depWaits.map((t) => t.wait?.started_at_unix_ms),
      ].filter((value) => typeof value === 'number' && Number.isFinite(value));
      const group = units.get(proj.group_id);
      const parent = group?.parent_id ? units.get(group.parent_id) : null;
      const latestRaw = data.updates.filter((u) => u.project_id === proj.project_id)
        .sort((left, right) => right.starts_on.localeCompare(left.starts_on)
          || right.recorded_at_unix_ms - left.recorded_at_unix_ms)[0] ?? null;
      let latestUpdate = null;
      if (latestRaw) {
        const startsOnMs = Date.parse(`${latestRaw.starts_on}T00:00:00Z`);
        const weeksBeforeFixtureNow = Math.round((fixtureWeekStart - startsOnMs) / WEEK);
        const startsAt = currentWeekStart - weeksBeforeFixtureNow * WEEK;
        latestUpdate = {
          update_id: latestRaw.update_id,
          period_kind: latestRaw.period_kind,
          period_key: latestRaw.period_key,
          starts_on: isoUtcDate(startsAt),
          ends_on: isoUtcDate(startsAt + WEEK - DAY),
          status: latestRaw.status,
          summary: latestRaw.summary,
          author: person(latestRaw.author_person_id),
          recorded_at_unix_ms: latestRaw.recorded_at_unix_ms,
        };
      }
      return {
        project_id: proj.project_id,
        project_key: proj.key,
        project_status: 'active',
        coordinator_epoch: `epoch-${proj.project_id}-47`,
        project_revision: '47',
        responsible_person: null,
        directory: {
          registered: Boolean(group),
          group: group ? { id: group.id, name: group.name } : null,
          departments: parent ? [{ id: parent.id, name: parent.name }] : [],
        },
        works: {
          total: tasks.length,
          not_started: notStarted,
          in_progress: inProgress,
          delivered_unverified: deliveredUnverified,
          verified,
          completed_without_independent_review: 0,
          retired: 0,
          hidden_work_count: 0,
        },
        waits: {
          decision: openDecisions.length,
          review: openReviews.length,
          dependency: depWaits.length,
          oldest_open_unix_ms: openTimes.length ? Math.min(...openTimes) : null,
        },
        latest_update: latestUpdate,
        read: { status: 'complete' },
      };
    });
    const paged = page(items, query, 200);
    return {
      protocol_version: 1,
      op: 'portfolio.overview',
      selection_basis: 'member',
      data: {
        as_of: { at_unix_ms: data.now_unix_ms },
        execution_authorized: false,
        projects: paged.items,
        total_projects: paged.total,
        truncated: paged.truncated,
        next_cursor: paged.next_cursor,
      },
    };
  }

  return {
    variant,
    data,
    query: read,
    upstream,
    directory,
    connections,
    revokeConnection,
    portfolioOverview,
    projects: () => clone(data.projects),
    session: () => ({
      ok: true,
      session_id: 'demo-anonymous',
      created_at_ms: data.now_unix_ms - 60_000,
      last_seen_ms: data.now_unix_ms,
      expires_at_ms: data.now_unix_ms + 3600000,
      projects: data.projects.map((item) => item.key),
    }),
  };
}

module.exports = {
  createDemoOrganization, DemoReadError, DEMO_VARIANTS, DEMO_FUTURE_READ_OPS,
  canonicalOverviewInput, demoOverviewAsOf, canonicalProjectList, canonicalPeriodUpdates,
  canonicalOrganizationDirectory, canonicalPerspectiveFacts, canonicalDepartmentFacts,
};
