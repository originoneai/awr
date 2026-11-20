'use strict';

const assert = require('node:assert/strict');
const { after, before, test } = require('node:test');
const { createServer } = require('vite');
const { createTeamBridge } = require('../server/team-bridge');
const { createDemoOrganization, DEMO_FUTURE_READ_OPS } = require('../server/demo-organization');
const fixture = require('../fixtures/demo-organization.json');

const DAY = 86_400_000;
const clone = (value) => structuredClone(value);
const complete = (items) => ({ coverage: 'complete', items });
const statusOf = (task) => task.prototype_status;

function canonicalInput(data) {
  const activity = data.tasks.flatMap((task) => task.timeline.map((entry) => ({
    id: entry.activity_id,
    taskId: task.work_id,
    action: 'session.checkpoint',
    actorId: `${task.owner_person_id}-agent`,
    atUnixMs: entry.at_unix_ms,
  })));
  return {
    tasks: complete(data.tasks.map((task) => ({
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
    dependencies: complete(data.tasks.flatMap((task) => task.dependencies.map((dependency) => ({
      id: dependency.dependency_id,
      consumerTaskId: task.work_id,
      providerTaskId: dependency.provider_work_id,
      satisfied: dependency.satisfied,
      what: dependency.required_version_label,
      sinceUnixMs: dependency.since_unix_ms,
    })))),
    decisions: complete(data.decisions.map((decision) => ({
      id: decision.decision_id,
      taskId: decision.work_id,
      state: decision.state,
      current: decision.current,
      question: decision.question,
      impact: decision.impact,
      allowedResponderIds: decision.allowed_responder_person_ids,
      createdAtUnixMs: decision.created_at_unix_ms,
    }))),
    reviewRounds: complete(data.tasks.flatMap((task) => (task.delivery?.rounds || []).map((round) => ({
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
    handoffs: complete(data.handoffs.map((handoff) => ({
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
    // maps one-to-one onto OverviewApproval; assertFixtureReferences pins every one to its
    // pending record, its lesson or planning candidate, and its recipient and project.
    approvals: complete(data.approvals.map((approval) => ({
      id: approval.id,
      kind: approval.kind,
      recipientPersonId: approval.recipientPersonId,
      projectId: approval.projectId,
      title: approval.title,
      sinceUnixMs: approval.sinceUnixMs,
    }))),
    members: complete([...data.people, ...data.view_roles].map((member) => ({
      personId: member.person_id,
      displayName: member.display_name,
      groupId: member.group_id,
    }))),
    activity: complete(activity),
    options: {
      nowUnixMs: data.now_unix_ms,
      stalledAfterDays: 1,
      views: {
        me: { personId: 'M-002', projectId: 'P-001' },
        lead: { personId: 'M-001', projectId: 'P-001' },
        department: { personId: 'M-010', projectIds: ['P-001', 'P-002', 'P-003'] },
        top: { personId: 'M-011', goalIds: ['GOAL-001', 'GOAL-002', 'GOAL-003'] },
        currentWeek: { startUnixMs: Date.parse('2026-11-16T00:00:00Z'), endUnixMs: Date.parse('2026-11-22T23:59:59.999Z') },
        previousWeek: { startUnixMs: Date.parse('2026-11-09T00:00:00Z'), endUnixMs: Date.parse('2026-11-15T23:59:59.999Z') },
        currentMonth: { startUnixMs: Date.parse('2026-11-01T00:00:00Z'), endUnixMs: Date.parse('2026-11-30T23:59:59.999Z') },
        previousMonth: { startUnixMs: Date.parse('2026-10-01T00:00:00Z'), endUnixMs: Date.parse('2026-10-31T23:59:59.999Z') },
      },
    },
  };
}

function assertFixtureReferences(data) {
  const taskIds = new Set(data.tasks.map((task) => task.work_id));
  const projectIds = new Set(data.projects.map((project) => project.project_id));
  const personIds = new Set(data.people.map((person) => person.person_id));
  for (const task of data.tasks) {
    assert.ok(projectIds.has(task.project_id), `${task.work_id} has a known project`);
    assert.ok(personIds.has(task.owner_person_id), `${task.work_id} has a known owner`);
    for (const dependency of task.dependencies) {
      assert.ok(taskIds.has(dependency.provider_work_id), `${dependency.dependency_id} has a known provider`);
      const provider = data.tasks.find((item) => item.work_id === dependency.provider_work_id);
      if (dependency.satisfied) {
        assert.equal(statusOf(provider), 'verified',
          `${dependency.dependency_id} cannot be satisfied by an unverified provider`);
      }
    }
    if (statusOf(task) === 'verified') {
      assert.ok(task.integrated, `${task.work_id} is integrated`);
      assert.ok(task.acceptance.every((criterion) => criterion.evidence_state === 'ok'),
        `${task.work_id} has evidence for every criterion`);
      const latest = task.delivery.rounds.at(-1);
      assert.equal(latest.state, 'approved');
      assert.equal(latest.current, true);
      assert.equal(latest.independent, true);
      assert.notEqual(latest.author_person_id, latest.reviewer_person_id);
    }
  }
  const lessonById = new Map(data.lessons.map((lesson) => [lesson.lesson_id, lesson]));
  const pendingById = new Map(data.pending.map((item) => [item.pending_id, item]));
  const approvalIds = new Set();
  for (const approval of data.approvals) {
    assert.match(approval.id, /^A-\d{3}$/, 'an approval id follows the opaque pending-key style');
    assert.ok(!approvalIds.has(approval.id), `${approval.id} is used by one approval only`);
    approvalIds.add(approval.id);
    assert.ok(['plan-approval', 'lesson-confirmation'].includes(approval.kind),
      `${approval.id} has a known approval kind`);
    assert.ok(personIds.has(approval.recipientPersonId), `${approval.id} has a known recipient`);
    assert.ok(projectIds.has(approval.projectId), `${approval.id} has a known project`);
    const pending = pendingById.get(approval.id);
    assert.ok(pending, `${approval.id} traces to a pending item with the same key`);
    const pendingKind = { 'plan-approval': '批准拆解', 'lesson-confirmation': '确认经验' }[approval.kind];
    assert.equal(pending.kind, pendingKind, `${approval.id} matches its pending item's kind`);
    assert.equal(pending.recipient_person_id, approval.recipientPersonId,
      `${approval.id} matches its pending item's recipient`);
    assert.equal(pending.project_id, approval.projectId, `${approval.id} matches its pending item's project`);
    assert.equal(pending.created_at_unix_ms, approval.sinceUnixMs,
      `${approval.id} keeps its pending item's creation time`);
    if (approval.kind === 'lesson-confirmation') {
      const lesson = lessonById.get(pending.lesson_id);
      assert.ok(lesson, `${approval.id} traces to an existing lesson candidate`);
      assert.equal(lesson.pending_id, approval.id,
        `${approval.id} is the lesson candidate's own pending confirmation`);
      assert.equal(lesson.stage, 'candidate', `${approval.id} confirms a candidate lesson`);
      assert.equal(lesson.state, 'active', `${approval.id} confirms an active lesson`);
      assert.equal(lesson.author_person_id, approval.recipientPersonId,
        `${approval.id} is confirmed by the lesson's author`);
      assert.equal(lesson.title, approval.title,
        `${approval.id} carries the lesson's product-language title`);
    } else {
      assert.ok(pending.planning_candidate_id && data.planning_candidates.some((candidate) =>
        candidate.candidate_id === pending.planning_candidate_id),
      `${approval.id} traces to an existing planning candidate`);
    }
  }
  for (const item of data.pending) {
    if (item.kind === '确认经验' || item.kind === '批准拆解') {
      assert.ok(approvalIds.has(item.pending_id),
        `${item.pending_id} (kind ${item.kind}) is mapped into the approvals plane`);
    }
  }
}

let vite;
let deriveOverview;
before(async () => {
  vite = await createServer({ configFile: false, server: { middlewareMode: true }, appType: 'custom', logLevel: 'silent' });
  ({ deriveOverview } = await vite.ssrLoadModule('/src/core/overview/index.ts'));
});

after(async () => {
  await vite?.close();
});

test('the public fixture has the exact organization cardinalities and no live-run material', () => {
  assert.deepEqual({
    projects: fixture.projects.length,
    people: fixture.people.length,
    tasks: fixture.tasks.length,
    documents: fixture.documents.length,
    updates: fixture.updates.length,
    goals: fixture.goals.length,
    lessons: fixture.lessons.length,
    pending: fixture.pending.length,
    approvals: fixture.approvals.length,
  }, { projects: 3, people: 8, tasks: 29, documents: 9, updates: 8, goals: 3, lessons: 4, pending: 7, approvals: 2 });
  assert.equal(fixture.public_synthetic, true);
  assertFixtureReferences(fixture);
  const text = JSON.stringify(fixture);
  for (const forbidden of [
    /awr1\./i, /Bearer\s+[A-Za-z0-9._-]{12,}/i, /BEGIN [A-Z ]*PRIVATE KEY/i,
    /\/Users\//i, /\.local\//i, /originone/i, /support-queue/i, /issue-filter/i,
    /refs\/heads\//i, /customer-support/i, /native-2026/i, /aw[r]?_[a-z]*session=/i,
  ]) assert.doesNotMatch(text, forbidden);
  assert.doesNotMatch(text, /(?<![0-9a-f])[0-9a-f]{40}(?![0-9a-f])/i,
    'the fixture must not contain a commit-shaped revision');
});

test('opaque keys are separate from every display title', () => {
  for (const task of fixture.tasks) {
    assert.match(task.work_id, /^T-\d{3}$/);
    assert.match(task.external_key, /^T-\d{3}$/);
    assert.notEqual(task.work_id, task.title);
    assert.notEqual(task.external_key, task.title);
    assert.match(task.workstream_id, /^WS-\d{3}$/);
    assert.ok(task.title.length > 0);
  }
  for (const project of fixture.projects) {
    assert.match(project.key, /^P-\d{3}$/);
    assert.match(project.project_id, /^P-\d{3}$/);
    assert.notEqual(project.key, project.title);
  }
  for (const member of fixture.people) {
    assert.match(member.person_id, /^M-\d{3}$/);
    assert.notEqual(member.person_id, member.display_name);
  }
  for (const decision of [...fixture.decisions, ...fixture.project_questions]) {
    assert.match(decision.decision_id, /^Q-\d{3}$/);
    assert.notEqual(decision.decision_id, decision.question);
  }
  for (const document of fixture.documents) {
    assert.match(document.document_id, /^DOC-\d{3}$/);
    assert.equal(document.path, `docs/${document.project_id}/${document.document_id}.md`);
    assert.notEqual(document.path, document.name);
  }
  for (const goal of fixture.goals) {
    assert.match(goal.goal_id, /^GOAL-\d{3}$/);
    assert.notEqual(goal.goal_id, goal.title);
  }
  for (const role of fixture.view_roles) {
    assert.match(role.person_id, /^M-\d{3}$/);
    assert.match(role.group_id, /^G-\d{3}$/);
  }
  for (const approval of fixture.approvals) {
    assert.match(approval.id, /^A-\d{3}$/);
    assert.notEqual(approval.id, approval.title);
  }
});

test('PPO-010 derives the prototype statuses and the revised prototype metric goldens', () => {
  const result = deriveOverview(canonicalInput(fixture));
  // The prototype's task states are the reference and the derivation must reproduce them
  // exactly: every task carries its prototype_status, nothing may override it, and each
  // derived status must equal it. This is the check that catches data edits made to move
  // a derived number instead of leaving the states as the prototype defines them.
  assert.ok(fixture.tasks.every((task) => typeof task.prototype_status === 'string'));
  assert.ok(fixture.tasks.every((task) => !('demo_status' in task)),
    'no task may override its prototype_status with a demo status');
  assert.deepEqual(Object.fromEntries(result.tasks.map((task) => [task.id, task.status])),
    Object.fromEntries(fixture.tasks.map((task) => [task.work_id, task.prototype_status])));
  assert.equal(fixture.tasks.find((task) => task.work_id === 'T-103').prototype_status, 'waiting');
  assert.equal(fixture.tasks.find((task) => task.work_id === 'T-114').prototype_status, 'claimed');
  assert.equal(result.taskCounts.all.count, 29);
  assert.equal(result.coverage.complete, true);
  assert.equal(result.coverage.sections.status.complete, true);
  assert.equal(result.coverage.sections.waits.complete, true);
  assert.equal(result.coverage.sections.views.complete, true);
  assert.equal(result.waitCounts.all.count, 8);
  assert.deepEqual(Object.fromEntries(Object.entries(result.waitCounts.byKind).map(([kind, value]) => [kind, value.count])), {
    decision: 2, dependency: 1, review: 3, external: 2,
  });
  assert.equal(result.chains.length, 5);
  const review = result.chains.find((chain) => chain.root.kind === 'review');
  assert.equal(review.root.queue.count, 3);
  assert.equal(review.impact.count, 5);
  const emptyState = result.waits.find((wait) => wait.kind === 'decision' && wait.task.id === 'T-104');
  assert.equal(emptyState.behind.count, 1);
  assert.deepEqual(emptyState.behind.items.map((task) => task.id), ['T-104']);

  function visit(value) {
    if (Array.isArray(value)) return value.forEach(visit);
    if (!value || typeof value !== 'object') return;
    if (Array.isArray(value.items) && Array.isArray(value.unknownItems) && 'count' in value) {
      if (value.count !== null) assert.equal(value.count, value.items.length, 'a count and its details share one array');
    }
    Object.values(value).forEach(visit);
  }
  visit(result);

  const metrics = Object.fromEntries(Object.entries(result.views).map(([view, summary]) =>
    [view, summary.metrics.map((metric) => ({ count: metric.count, details: metric.items.length }))]));
  assert.deepEqual(metrics.me, [{ count: 2, details: 2 }, { count: 1, details: 1 }, { count: 2, details: 2 }, { count: 3, details: 3 }]);
  assert.deepEqual(metrics.lead, [{ count: 2, details: 2 }, { count: 7, details: 7 }, { count: 7, details: 7 }, { count: 2, details: 2 }]);
  assert.deepEqual(metrics.department, [{ count: 3, details: 3 }, { count: 10, details: 10 }, { count: 5, details: 5 }, { count: 0, details: 0 }]);
  assert.deepEqual(metrics.top, [{ count: 3, details: 3 }, { count: 12, details: 12 }, { count: 5, details: 5 }, { count: 0, details: 0 }]);
  assert.equal(result.views.lead.metrics[0].previousPeriod.count, 2);
  assert.equal(result.views.department.metrics[1].previousPeriod.count, 2);
  assert.equal(result.views.top.metrics[1].claimed.count, 2);
  assert.equal(fixture.people.length, 8);
  assert.equal(fixture.view_roles.length, 2);
  assert.equal(result.peopleCounts.all.count, 10);

  // Prototype comparison: the me and lead rows now match the prototype exactly — the
  // running-Agent metric counts a waiting task whose Agent is explicitly active (T-103,
  // not the stalled T-104 or the inactive waiting T-111), and T-104's self-addressed decision
  // belongs to Codex under waiterOf rather than to 林悦's waiting-on-others list. The approvals
  // plane fills the pending queues. Department and top still differ on the prototype's primary-wait
  // projection (the third number) and its project-level questions, which have no input
  // plane yet (the fourth number).
  assert.deepEqual({
    me: { prototype: [2, 1, 2, 3], derived: metrics.me.map((metric) => metric.count) },
    lead: { prototype: [2, 7, 7, 2], derived: metrics.lead.map((metric) => metric.count) },
    department: { prototype: [3, 10, 2, 1], derived: metrics.department.map((metric) => metric.count) },
    top: { prototype: [3, 12, 2, 1], derived: metrics.top.map((metric) => metric.count) },
  }, {
    me: { prototype: [2, 1, 2, 3], derived: [2, 1, 2, 3] },
    lead: { prototype: [2, 7, 7, 2], derived: [2, 7, 7, 2] },
    department: { prototype: [3, 10, 2, 1], derived: [3, 10, 5, 0] },
    top: { prototype: [3, 12, 2, 1], derived: [3, 12, 5, 0] },
  });
  // The prototype's inbox composes 林悦's three pending items from three different planes —
  // the decision her Codex asks about (Q-001), 周宁's assignment (H-001) and the candidate
  // lesson awaiting her own confirmation (A-002) — and 周宁's two from the decision on
  // T-105 (Q-002) plus the plan breakdown awaiting his approval (A-004). The department
  // and top viewers receive no approval and no task-scoped decision, so their pending
  // queues stay 0 against the prototype's project-level questions (no input plane yet).
  const pendingComposition = (view) => result.views[view].metrics[3].items.map((item) =>
    item.kind === 'approval' ? `approval:${item.approval.kind}`
      : item.kind === 'wait' ? `wait:${item.wait.kind}` : item.kind);
  assert.deepEqual(pendingComposition('me'), ['wait:decision', 'handoff', 'approval:lesson-confirmation']);
  assert.deepEqual(pendingComposition('lead'), ['wait:decision', 'approval:plan-approval']);
  assert.deepEqual(result.views.me.metrics[3].items.filter((item) => item.kind === 'approval')
    .map((item) => item.approval.id), ['A-002']);
  assert.deepEqual(result.views.lead.metrics[3].items.filter((item) => item.kind === 'approval')
    .map((item) => item.approval.id), ['A-004']);
  assert.deepEqual([...new Set(fixture.approvals.map((approval) => approval.kind))].sort(),
    ['lesson-confirmation', 'plan-approval']);
});

test('a planted dependency contradiction fails both the structural and PPO-010 oracles', () => {
  const mutant = clone(fixture);
  const consumer = mutant.tasks.find((task) => task.work_id === 'T-106');
  consumer.dependencies[0].satisfied = true;
  assert.throws(() => assertFixtureReferences(mutant), /cannot be satisfied by an unverified provider/);
  const result = deriveOverview(canonicalInput(mutant));
  assert.notEqual(result.tasks.find((task) => task.id === 'T-106').status,
    consumer.prototype_status,
    'the planted contradiction must change the derived status and fail the status golden');
});

test('a planted agent flag that contradicts a prototype_status fails the status oracle', () => {
  // Re-planting the round-1 deviation (T-114's completion declaration withdrawn and its agent
  // flag raised against prototype_status 'claimed') is exactly the kind of edit that moves a
  // derived number (the me view's running Agents) by bending a task state; the
  // derived-vs-prototype status check must reject it.
  const mutant = clone(fixture);
  const task = mutant.tasks.find((item) => item.work_id === 'T-114');
  task.agent_active = true;
  task.report.completed_declaration = false;
  const result = deriveOverview(canonicalInput(mutant));
  assert.equal(result.tasks.find((item) => item.id === 'T-114').status, 'running');
  assert.notEqual(result.tasks.find((item) => item.id === 'T-114').status, task.prototype_status);
  assert.throws(() => assert.deepEqual(
    Object.fromEntries(result.tasks.map((item) => [item.id, item.status])),
    Object.fromEntries(mutant.tasks.map((item) => [item.work_id, item.prototype_status]))),
  /Expected values to be strictly deep-equal/);
});

test('emptying the fixture approvals fails the structural and the pending-metric oracles', () => {
  // Clearing the approvals plane is the pre-#694 state: every pending approval disappears from
  // the me and lead queues and their counts fall back below the prototype's 3 and 2.
  const mutant = clone(fixture);
  mutant.approvals = [];
  assert.throws(() => assertFixtureReferences(mutant), /is mapped into the approvals plane/);
  const result = deriveOverview(canonicalInput(mutant));
  assert.equal(result.views.me.metrics[3].count, 2);
  assert.equal(result.views.lead.metrics[3].count, 1);
  assert.throws(() => assert.deepEqual(
    [result.views.me.metrics[3].count, result.views.lead.metrics[3].count], [3, 2]),
  /Expected values to be strictly deep-equal/);
});

test('an approval addressed to a person outside the organization fails the structural oracle', () => {
  // The derivation accepts any non-empty recipient id (a ghost recipient is a person summary
  // nobody asked about, not a coverage gap), so only the fixture check against the
  // organization's membership can catch a dangling recipient_person_id.
  const mutant = clone(fixture);
  mutant.approvals[0].recipientPersonId = 'M-999';
  assert.throws(() => assertFixtureReferences(mutant), /has a known recipient/);
  const result = deriveOverview(canonicalInput(mutant));
  assert.equal(result.coverage.complete, true);
  assert.equal(result.views.me.metrics[3].count, 2,
    'the derivation alone does not drop the ghost approval from the me queue');
});

function bridgeForTest(options = {}) {
  const bridge = createTeamBridge({ demo: true, port: 0, ...options });
  const req = { headers: {}, method: 'POST' };
  const res = { setHeader() {} };
  return {
    bridge,
    call: (route, target, body) => bridge.routes[route](new URL(target, 'http://127.0.0.1'), body, req, res),
    read: (project, query) => bridge.routes['POST /api/team/read'](new URL('http://127.0.0.1/api/team/read'),
      { project, query: { protocol_version: 1, ...query } }, req, res),
  };
}

test('every exposed read operation answers through the real bridge validation path', async () => {
  const { read } = bridgeForTest();
  const review = await read('P-001', { op: 'review.inspect', work_id: 'T-101', review_round_id: 'round-t-101-2' });
  assert.equal(review.ok, true);
  const artifact = review.result.data.review;
  const document = fixture.documents.find((item) => item.project_id === 'P-001');
  const round = fixture.tasks.find((task) => task.work_id === 'T-101').delivery.rounds.at(-1);
  const queries = [
    { op: 'capabilities' }, { op: 'workstreams.list' }, { op: 'work.list', workstream_id: 'WS-002' },
    { op: 'work.graph', workstream_id: 'WS-002' }, { op: 'work.search', workstream_id: 'WS-002', search: '订单' },
    { op: 'planning.outcome', request_id: 'unknown-request' },
    { op: 'planning.candidates', limit: 5, max_context_bytes: 262144 },
    { op: 'audit.requests' }, { op: 'audit.development' },
    { op: 'task.assignees', work_id: 'T-101', workstream_id: 'WS-002' },
    { op: 'delivery.neutral.inspect', work_id: 'T-101', workstream_id: 'WS-002' },
    { op: 'delivery.submission.describe', work_id: 'T-101', workstream_id: 'WS-002' },
    { op: 'delivery.neutral.outcome', work_id: 'T-101', workstream_id: 'WS-002', request_id: 'unknown-request' },
    { op: 'delivery.source.status', work_id: 'T-101', workstream_id: 'WS-002' },
    { op: 'delivery.integration.inspect', work_id: 'T-101', workstream_id: 'WS-002', request_id: 'integration-round-t-101-2' },
    { op: 'work.prepare', work_id: 'T-101', workstream_id: 'WS-002' },
    { op: 'work.admission', work_id: 'T-101', workstream_id: 'WS-002' },
    { op: 'work.snapshot', work_id: 'T-101', workstream_id: 'WS-002' },
    { op: 'work.observe', work_id: 'T-101', workstream_id: 'WS-002' },
    { op: 'work.recovery', work_id: 'T-101', workstream_id: 'WS-002' },
    { op: 'command.inspect', work_id: 'T-101', workstream_id: 'WS-002', request_id: 'unknown-request' },
    { op: 'claim.inspect', work_id: 'T-101', workstream_id: 'WS-002', claim_id: round.claim_id },
    { op: 'execution.inspect', work_id: 'T-101', workstream_id: 'WS-002', execution_id: round.execution_id },
    { op: 'handoff.inspect', work_id: 'T-112', workstream_id: 'WS-001', handoff_id: 'H-001' },
    { op: 'evidence.inspect', work_id: 'T-101', workstream_id: 'WS-002', evidence_id: round.evidence_id },
    { op: 'review.rounds', work_id: 'T-101', workstream_id: 'WS-002' },
    { op: 'completion.inspect', work_id: 'T-101', workstream_id: 'WS-002' },
    { op: 'delivery.inspect', work_id: 'T-101', workstream_id: 'WS-002' },
    { op: 'artifact.content', work_id: 'T-101', artifact_id: round.artifact_id, expected_sha256: artifact.artifact_digest, max_context_bytes: 262144 },
    { op: 'artifact.segment', work_id: 'T-101', artifact_id: round.artifact_id, expected_sha256: artifact.artifact_digest, max_context_bytes: 1024 },
    { op: 'delivery.exports', work_id: 'T-101', workstream_id: 'WS-002' },
    { op: 'events.list', workstream_id: 'WS-002' }, { op: 'work.next' }, { op: 'work.inbox' },
    { op: 'sources.inspect', limit: 20, max_context_bytes: 262144 },
    { op: 'documents.list', limit: 20, max_context_bytes: 262144 },
    { op: 'decisions.list', decision_state: 'all' }, { op: 'handoff.list', handoff_state: 'all' },
    { op: 'dependencies.list', work_id: 'T-103', workstream_id: 'WS-001' },
    { op: 'session.inspect', session_id: 'session-t-101' },
    { op: 'source.content', source_path: document.path, expected_sha256: document.sha256, max_context_bytes: 262144 },
    ...[...DEMO_FUTURE_READ_OPS].map((op) => op === 'update.inspect' ? { op, update_id: 'U-001' }
      : op === 'goal.inspect' ? { op, goal_id: 'GOAL-001' }
        : op === 'lesson.inspect' ? { op, lesson_id: 'L-001' } : { op }),
  ];
  assert.equal(queries.length + 1, 48, '48 unique reads, including the seeded review.inspect');
  for (const query of queries) {
    const response = await read('P-001', query);
    assert.equal(response.ok, true, `${query.op}: ${response.error?.code || ''} ${response.error?.message || ''}`);
    assert.notEqual(response.error?.code, 'Unsupported', query.op);
  }
});

test('all projects, task details, historical outcomes and document bodies remain reachable', async () => {
  const { call, read } = bridgeForTest();
  const projects = await call('GET /api/team/projects', '/api/team/projects');
  assert.equal(projects.ok, true);
  assert.equal(projects.projects.length, 3);
  for (const project of fixture.projects) {
    const overview = await call('GET /api/team/overview', `/api/team/overview?project=${project.key}`);
    assert.equal(overview.ok, true, project.key);
    assert.equal(overview.works.length, project.key === 'P-001' ? 14 : project.key === 'P-002' ? 8 : 7);
    const graph = await call('GET /api/team/graph', `/api/team/graph?project=${project.key}`);
    assert.equal(graph.ok, true, `${project.key} graph`);
    const directory = await call('POST /api/team/directory', '/api/team/directory', { project: project.key });
    assert.equal(directory.ok, true, `${project.key} directory`);
    const connections = await call('POST /api/team/connections/list', '/api/team/connections/list', { project: project.key });
    assert.equal(connections.ok, true, `${project.key} connections`);
  }
  for (const task of fixture.tasks) {
    const snapshot = await read(task.project_id, { op: 'work.snapshot', work_id: task.work_id, workstream_id: task.workstream_id });
    assert.equal(snapshot.ok, true, `${task.work_id} snapshot: ${snapshot.error?.message || ''}`);
    const detail = await call('GET /api/team/work',
      `/api/team/work?project=${task.project_id}&work=${task.work_id}&workstream=${task.workstream_id}`);
    assert.equal(detail.ok, true, `${task.work_id} detail: ${detail.error?.message || ''}`);
    for (const round of task.delivery?.rounds || []) {
      for (const query of [
        { op: 'review.inspect', work_id: task.work_id, review_round_id: round.round_id },
        { op: 'evidence.inspect', work_id: task.work_id, evidence_id: round.evidence_id },
        { op: 'execution.inspect', work_id: task.work_id, execution_id: round.execution_id },
        { op: 'claim.inspect', work_id: task.work_id, claim_id: round.claim_id },
      ]) {
        const response = await read(task.project_id, query);
        assert.equal(response.ok, true, `${task.work_id} ${query.op} ${round.round_id}: ${response.error?.message || ''}`);
      }
    }
  }
  for (const document of fixture.documents) {
    const response = await read(document.project_id, {
      op: 'source.content', source_path: document.path, expected_sha256: document.sha256, max_context_bytes: 262144,
    });
    assert.equal(response.ok, true, document.path);
    assert.equal(response.result.data.text, document.body);
  }
});

test('update facts and lesson usage are computed from their detail arrays', async () => {
  const { read } = bridgeForTest();
  const expectedFactCounts = {
    'U-001': [2, 6, 0, 1, 1], 'U-002': [2, 1, 1, 1], 'U-003': [1, 0],
    'U-004': [0, 0], 'U-005': [1, 1], 'U-006': [3], 'U-007': [2, 1], 'U-008': [1],
  };
  for (const update of fixture.updates) {
    const countable = update.fact_snapshot.facts.filter((fact) => fact.item_ids !== null);
    assert.deepEqual(countable.map((fact) => fact.item_ids.length), expectedFactCounts[update.update_id]);
    for (const fact of update.fact_snapshot.facts) {
      if (fact.item_ids === null) assert.equal(typeof fact.value_text, 'string');
      else assert.equal(fact.value_text, null);
    }
    const response = await read(update.project_id, { op: 'update.inspect', update_id: update.update_id });
    assert.equal(response.ok, true);
    assert.equal(response.result.data.update.fact_snapshot_ref, update.fact_snapshot.fact_snapshot_ref);
  }
  for (const lesson of fixture.lessons) {
    const response = await read(lesson.project_id, { op: 'lesson.inspect', lesson_id: lesson.lesson_id });
    assert.equal(response.ok, true);
    const record = response.result.data.lesson;
    assert.equal(record.usage_count, lesson.usage_records.length);
    assert.equal(record.passed_count, lesson.usage_records.filter((item) => item.outcome === 'passed').length);
    assert.equal(record.not_applicable_count,
      lesson.usage_records.filter((item) => item.outcome === 'not_applicable').length);
    assert.equal(record.usage_count, record.passed_count + record.not_applicable_count);
    assert.equal('per_person_ranking' in record, false);
  }
});

test('empty, single, restricted and partial variants expose honest state envelopes', async () => {
  const empty = bridgeForTest({ demoVariant: 'empty' });
  const emptyProjects = await empty.call('GET /api/team/projects', '/api/team/projects');
  assert.deepEqual(emptyProjects.projects, []);
  assert.equal((await empty.read('P-001', { op: 'workstreams.list' })).error.code, 'Forbidden');

  const single = bridgeForTest({ demoVariant: 'single' });
  const singleProjects = await single.call('GET /api/team/projects', '/api/team/projects');
  assert.deepEqual(singleProjects.projects.map((project) => project.key), ['P-001']);
  const singleOverview = await single.call('GET /api/team/overview', '/api/team/overview?project=P-001');
  assert.equal(singleOverview.ok, true);
  assert.equal(singleOverview.works.length, 14);
  assert.equal(single.bridge.TEAM.demoOrganization.data.people.length, 4);
  assert.equal(single.bridge.TEAM.demoOrganization.data.pending.length, 6);
  assert.equal(single.bridge.TEAM.demoOrganization.data.approvals.length, 2);

  const restricted = bridgeForTest({ demoVariant: 'restricted' });
  assert.equal((await restricted.call('GET /api/team/overview', '/api/team/overview?project=P-003')).error.code, 'Forbidden');
  assert.equal((await restricted.read('P-001', { op: 'documents.list' })).error.code, 'Forbidden');
  assert.equal((await restricted.call('POST /api/team/directory', '/api/team/directory', { project: 'P-001' })).error.code, 'Forbidden');
  assert.equal(JSON.stringify(await restricted.read('P-001', { op: 'documents.list' })).includes('订单协作平台的目标'), false);

  const partial = bridgeForTest({ demoVariant: 'partial' });
  const overview = await partial.call('GET /api/team/overview', '/api/team/overview?project=P-001');
  assert.equal(overview.ok, true);
  assert.equal(overview.coverage.failed, 1);
  assert.equal(overview.coverage.succeeded, 2);
  assert.equal(overview.coverage.works_enumerated, 9);
  assert.equal(overview.coverage.works_total, null);
  assert.equal(overview.coverage.truncated, false);
  assert.equal(overview.coverage.streams.find((part) => part.status === 'failed').error_code, 'Unavailable');
  assert.equal((await partial.read('P-001', { op: 'work.snapshot', work_id: 'T-102', workstream_id: 'WS-002' })).error.code, 'Unavailable');
  assert.equal((await partial.call('GET /api/team/activity', '/api/team/activity?project=P-001&kind=development')).error.code, 'Unavailable');
  assert.equal((await partial.call('GET /api/team/graph', '/api/team/graph?project=P-001')).error.code, 'Unavailable');
});

test('the organization demo never issues business receipts and future ops stay out of the live registry', async () => {
  const { call } = bridgeForTest();
  const action = await call('POST /api/team/action', '/api/team/action', {
    project: 'P-001', work_key: 'T-101', action: 'accept', request_id: 'synthetic-request',
  });
  assert.equal(action.ok, false);
  assert.equal(action.error.code, 'Unsupported');
  const live = bridgeForTest({ teamUrl: 'http://127.0.0.1:1', demo: false });
  const future = await live.read('P-001', { op: 'goals.list' });
  assert.equal(future.ok, false);
  assert.equal(future.error.code, 'Unsupported');
  assert.throws(() => createDemoOrganization({ variant: 'unknown' }), /Unknown demo variant/);
});

const { overviewTaskFromList, overviewMemberFromDirectory } = require('../server/team-bridge');

// Verbatim value shapes from crates/awr-team-pg/src/workstream_read/list.rs:108-110.
// The integration states come from the delivery_integration_intents check constraint.
// live-samples/ has no work.list summary, so these constants deliberately follow Rust.
const RUST_CONFIRMED_OUTCOME = {
  summary: '加了缓存层之后压测达标，已经通过复核。',
  completed: ['加缓存层和失效策略', '压测达标'],
  integration: {
    id: 'integration-round-t-101-2', state: 'confirmed', reviewed_execution_id: 'execution-t-101-2',
    current_binding: true, target_changed: false,
  },
};
const RUST_PREPARED_OUTCOME = {
  summary: '第 3 版已提交复核，补上了超时重发的处理。',
  completed: ['补重复提交的判断', '补错误码对照表'],
  integration: {
    id: 'integration-round-t-102-1', state: 'prepared', reviewed_execution_id: 'execution-t-102-1',
    current_binding: true, target_changed: false,
  },
};
const RUST_NO_DELIVERY_OUTCOME = {
  summary: '列表分页已接通，正在补错误提示。提交流程在等接口第 3 版。',
  completed: ['列表分页接通'],
  integration: null,
};

function allDemoWorkItems(demo) {
  return demo.data.projects.flatMap((project) => project.workstreams.flatMap((stream) =>
    demo.query(project.project_id, {
      protocol_version: 1, op: 'work.list', workstream_id: stream.id,
    }).data.items));
}

async function overviewModelForTest(options = {}, project = 'P-001') {
  const { call } = bridgeForTest(options);
  const model = await call('GET /api/team/overview-model', `/api/team/overview-model?project=${project}`);
  assert.equal(model.ok, true, JSON.stringify(model));
  return model;
}

test('demo work.list uses the Rust outcome shapes and every available task fact survives the adapter', () => {
  const demo = createDemoOrganization({ variant: 'full', timestampShiftMs: 0 });
  const items = allDemoWorkItems(demo);
  const byId = new Map(items.map((item) => [item.work_id, item]));
  assert.equal(items.length, fixture.tasks.length);
  assert.deepEqual(byId.get('T-101').summary.outcome.value, RUST_CONFIRMED_OUTCOME);
  assert.deepEqual(byId.get('T-102').summary.outcome.value, RUST_PREPARED_OUTCOME);
  assert.deepEqual(byId.get('T-103').summary.outcome.value, RUST_NO_DELIVERY_OUTCOME);

  const adapted = items.map((item) => overviewTaskFromList(item, item.work_id.startsWith('T-1') ? 'P-001'
    : item.work_id.startsWith('T-2') ? 'P-002' : 'P-003', item.workstream_id));
  const adaptedById = new Map(adapted.map((task) => [task.id, task]));
  for (const source of fixture.tasks) {
    const task = adaptedById.get(source.work_id);
    assert.equal(task.started, source.started, `${source.work_id} started`);
    if (source.report) {
      assert.equal(task.agentName, source.report.reporter, `${source.work_id} Agent name`);
      assert.notEqual(task.integrated, undefined, `${source.work_id} integration fact`);
    }
  }
  assert.equal(adapted.filter((task) => task.integrated === true).length,
    fixture.tasks.filter((task) => statusOf(task) === 'verified').length,
    'the integrated tasks are exactly the prototype-verified tasks');
  assert.equal(adaptedById.get('T-101').integrated, true);
  assert.equal(adaptedById.get('T-102').integrated, false);
  assert.equal(adaptedById.get('T-103').integrated, null);
});

test('summary.execution maps active, ended and no_session without guessing when the field is absent', () => {
  const demo = createDemoOrganization({ variant: 'full', timestampShiftMs: 0 });
  const byId = new Map(allDemoWorkItems(demo).map((item) => [item.work_id, item]));
  const active = byId.get('T-103');
  assert.deepEqual(active.summary.execution, {
    value: { session_state: 'active', last_checkpoint_at_unix_ms: 1795088880000 },
    as_of_unix_ms: fixture.now_unix_ms,
    reason: null,
  });
  assert.equal(overviewTaskFromList(active, 'P-001', 'WS-001').agentActive, true);
  assert.equal(overviewTaskFromList(byId.get('T-101'), 'P-001', 'WS-002').agentActive, false);
  assert.deepEqual(byId.get('T-106').summary.execution, {
    value: null, as_of_unix_ms: null, reason: 'no_session',
  });
  assert.equal(overviewTaskFromList(byId.get('T-106'), 'P-001', 'WS-003').agentActive, false);

  const noExecution = clone(active);
  delete noExecution.summary.execution;
  const unknown = overviewTaskFromList(noExecution, 'P-001', 'WS-001');
  assert.equal(Object.hasOwn(unknown, 'agentActive'), false,
    'report and checkpoint times are not Agent-activity facts');
});

test('only the full demo publishes approvals and the demo viewer scope, while member actor ids survive', async () => {
  const model = await overviewModelForTest({ demoTimestampShiftMs: 0, now: () => fixture.now_unix_ms + 9 * DAY });
  assert.deepEqual(model.input.options.nowUnixMs, fixture.now_unix_ms);
  assert.deepEqual(model.input.options.views.me, { personId: fixture.viewer.person_id, projectId: 'P-001' });
  assert.deepEqual(model.input.approvals, complete(fixture.approvals));
  assert.equal(model.coverage.approvals.status, 'complete');
  const member = model.input.members.items.find((item) => item.personId === 'M-002');
  assert.deepEqual(member.actorIds, ['M-002', 'M-002-agent']);

  const liveMember = overviewMemberFromDirectory({
    member_ref: { project_id: 'synthetic-project', person_id: 'synthetic-person' },
    display_name: 'Synthetic Member', department_ids: ['synthetic-department'],
  });
  assert.deepEqual(liveMember, {
    personId: 'synthetic-person', displayName: 'Synthetic Member', groupId: 'synthetic-department',
  });
  assert.equal(Object.hasOwn(liveMember, 'actorIds'), false);
  assert.deepEqual(overviewMemberFromDirectory({
    member_ref: { project_id: 'P-001', person_id: 'M-002' }, display_name: '林悦',
    department_ids: ['G-001'], actor_ids: ['M-002', 'M-002-agent'],
  }).actorIds, ['M-002', 'M-002-agent']);

  for (const variant of ['partial', 'single']) {
    const degraded = await overviewModelForTest({ demoVariant: variant, demoTimestampShiftMs: 0,
      now: () => fixture.now_unix_ms });
    assert.deepEqual(degraded.input.approvals, { coverage: 'unread', reason: 'not-read' }, variant);
  }
  const restricted = bridgeForTest({ demoVariant: 'restricted', demoTimestampShiftMs: 0,
    now: () => fixture.now_unix_ms });
  const denied = await restricted.call('GET /api/team/overview-model', '/api/team/overview-model?project=P-002');
  assert.equal(denied.error.code, 'Forbidden');
  assert.equal(denied.input, undefined, 'restricted composition cannot expose approval facts');
});

test('demo work.next identity has no person_id and Agent audit records expose their client_id', () => {
  const demo = createDemoOrganization({ variant: 'full', timestampShiftMs: 0 });
  const next = demo.query('P-001', { protocol_version: 1, op: 'work.next' });
  assert.equal(Object.hasOwn(next.data.identity, 'person_id'), false);
  assert.equal(next.data.identity.actor_id, fixture.viewer.actor_id);
  assert.equal(next.data.identity.client_id, fixture.viewer.client_id);
  assert.deepEqual(next.data.identity.business_roles, fixture.viewer.business_roles);

  const audit = ['P-001', 'P-002'].flatMap((project) =>
    demo.query(project, { protocol_version: 1, op: 'audit.development' }).data.items);
  const byId = new Map(audit.map((item) => [item.id, item]));
  assert.equal(byId.get('EV-001').client_id, 'M-002-claude');
  assert.equal(byId.get('EV-009').client_id, 'M-002-codex');
  assert.equal(byId.get('EV-014').client_id, 'M-006-codex');
  for (const item of audit) assert.match(item.client_id, /^M-\d{3}-(?:claude|codex)$/);

  const humanDemo = createDemoOrganization({ variant: 'full', timestampShiftMs: 0 });
  humanDemo.data.activity[0].actor_id = 'M-002';
  const human = humanDemo.query('P-001', { protocol_version: 1, op: 'audit.development' }).data.items[0];
  assert.equal(human.client_id, null, 'a human actor has no Agent client id');
});

function numericTimes(value, path = 'root', found = []) {
  if (Array.isArray(value)) {
    value.forEach((item, index) => numericTimes(item, `${path}[${index}]`, found));
  } else if (value && typeof value === 'object') {
    for (const [key, item] of Object.entries(value)) {
      if (typeof item === 'number' && /(?:_ms|_unix_ms|UnixMs)$/.test(key)) found.push([`${path}.${key}`, item]);
      else numericTimes(item, `${path}.${key}`, found);
    }
  }
  return found;
}

function demoTimedReplies(demo) {
  return [
    demo.data,
    demo.query('P-001', { protocol_version: 1, op: 'work.list', workstream_id: 'WS-001' }),
    demo.query('P-001', { protocol_version: 1, op: 'work.next' }),
    demo.query('P-001', { protocol_version: 1, op: 'audit.development' }),
    demo.directory('P-001').json,
    demo.connections('P-001', {}).json,
    demo.session(),
  ];
}

test('one startup offset shifts every demo timestamp without changing relative time', () => {
  const shift = 47 * DAY + 1234;
  const baseline = createDemoOrganization({ variant: 'full', timestampShiftMs: 0 });
  const shifted = createDemoOrganization({ variant: 'full', timestampShiftMs: shift });
  assert.equal(shifted.data.now_unix_ms, fixture.now_unix_ms + shift);
  const beforeReplies = demoTimedReplies(baseline);
  const afterReplies = demoTimedReplies(shifted);
  let checked = 0;
  for (let index = 0; index < beforeReplies.length; index += 1) {
    const before = numericTimes(beforeReplies[index]);
    const after = numericTimes(afterReplies[index]);
    assert.deepEqual(after.map(([path]) => path), before.map(([path]) => path));
    for (let time = 0; time < before.length; time += 1) {
      assert.equal(after[time][1], before[time][1] + shift, `${after[time][0]} reply ${index}`);
      checked += 1;
    }
  }
  assert.ok(checked > 250, 'the check covers the organization and representative replies');
});

function shiftCanonicalTimes(value, shift) {
  if (Array.isArray(value)) return value.map((item) => shiftCanonicalTimes(item, shift));
  if (!value || typeof value !== 'object') return value;
  return Object.fromEntries(Object.entries(value).map(([key, item]) => [key,
    typeof item === 'number' && /UnixMs$/.test(key) ? item + shift : shiftCanonicalTimes(item, shift)]));
}

test('all four view metric sets are invariant under arbitrary demo clock offsets', () => {
  const expected = {
    me: [2, 1, 2, 3], lead: [2, 7, 7, 2], department: [3, 10, 5, 0], top: [3, 12, 5, 0],
  };
  const baselineInput = canonicalInput(fixture);
  for (const shift of [0, 47 * DAY + 1234, -80 * DAY, 400 * DAY]) {
    const result = deriveOverview(shiftCanonicalTimes(baselineInput, shift));
    const counts = Object.fromEntries(Object.entries(result.views).map(([view, summary]) =>
      [view, summary.metrics.map((metric) => metric.count)]));
    assert.deepEqual(counts, expected, `offset ${shift}`);
    assert.deepEqual(result.views.me.metrics[2].items.map((wait) => wait.task.id).sort(), ['T-103', 'T-111'],
      'the owner self-answer wait belongs to her Agent, not to the prototype waiting-on-others list');
    assert.ok(result.waits.every((wait) => wait.durationDays >= 0));
  }
});
