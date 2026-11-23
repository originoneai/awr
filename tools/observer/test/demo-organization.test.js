'use strict';

const assert = require('node:assert/strict');
const { after, before, test } = require('node:test');
const { createServer } = require('vite');
const { createTeamBridge } = require('../server/team-bridge');
const { projectDirectory } = require('../server/directory');
const {
  createDemoOrganization, DEMO_VARIANTS, DEMO_FUTURE_READ_OPS, canonicalOverviewInput,
} = require('../server/demo-organization');
const fixture = require('../fixtures/demo-organization.json');

const DAY = 86_400_000;
const WEEK = 7 * DAY;
const clone = (value) => structuredClone(value);
const complete = (items) => ({ coverage: 'complete', items });
const statusOf = (task) => task.prototype_status;

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
  const result = deriveOverview(canonicalOverviewInput(fixture));
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
  const result = deriveOverview(canonicalOverviewInput(mutant));
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
  const result = deriveOverview(canonicalOverviewInput(mutant));
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
  const result = deriveOverview(canonicalOverviewInput(mutant));
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
  const result = deriveOverview(canonicalOverviewInput(mutant));
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

test('demo directory IDs join the overview model and its hierarchy passes the real reply validator', () => {
  const demo = createDemoOrganization({ variant: 'full', timestampShiftMs: 0 });
  const overviewMembers = canonicalOverviewInput(demo.data).members.items;
  const overviewById = new Map(overviewMembers.map((member) => [member.personId, member]));
  for (const project of demo.data.projects) {
    const reply = demo.directory(project.project_id);
    assert.equal(reply.status, 200);
    const result = projectDirectory(reply.json.data);
    assert.notEqual(result, null, project.project_id);
    assert.deepEqual(new Set(result.members.map((member) => member.personId)), new Set(project.member_person_ids));
    for (const member of result.members) {
      const overviewMember = overviewById.get(member.personId);
      assert.ok(overviewMember, `${project.project_id} directory person exists in the overview model`);
      assert.equal(member.name, overviewMember.displayName);
      assert.ok(overviewMembers.some((candidate) => candidate.displayName === member.name && candidate.personId === member.personId),
        'the same display name resolves to the same stable person ID');
    }
    for (const department of result.departments) {
      const source = reply.json.data.plan.departments.find((item) => item.id === department.key);
      assert.equal(department.parentKey, source.parent_id ?? null);
      assert.notEqual(department.parentKey, department.key);
      if (department.parentKey !== null) {
        assert.ok(reply.json.data.plan.departments.some((item) => item.id === department.parentKey));
      }
    }
  }
  const first = demo.directory('P-001').json.data;
  assert.deepEqual(projectDirectory(first).departments, [{ key: 'G-001', name: '应用小组', parentKey: 'G-004' }]);
  const missingParent = clone(first);
  missingParent.plan.departments.find((department) => department.id === 'G-004').parent_id = 'missing';
  assert.equal(projectDirectory(missingParent), null);
});

test('only the canonical demo serves an organization directory with recorded duties and project memberships', async () => {
  const canonical = bridgeForTest({ demoVariant: 'canonical', demoTimestampShiftMs: 0,
    now: () => fixture.now_unix_ms });
  const organization = await canonical.call('POST /api/team/directory', '/api/team/directory', {});
  assert.equal(organization.ok, true, JSON.stringify(organization));
  assert.equal(organization.data.organization, fixture.organization.name);
  assert.deepEqual(organization.data.departments.map((department) => department.name), ['应用小组', '平台小组', '交付小组']);
  assert.deepEqual(organization.data.members.map((member) => member.name), fixture.people.map((member) => member.display_name));
  for (const member of organization.data.members) {
    const source = fixture.people.find((person) => person.person_id === member.personId);
    assert.equal(member.duty, source.role);
    assert.deepEqual(member.projects.map((project) => project.name), fixture.projects
      .filter((project) => project.member_person_ids.includes(member.personId)).map((project) => project.title));
  }
  assert.deepEqual(organization.data.members.find((member) => member.personId === 'M-004').projects
    .map((project) => project.name), ['订单协作平台', '统一通知服务']);

  const selected = await canonical.call('POST /api/team/directory', '/api/team/directory', { project: 'P-001' });
  assert.equal(selected.ok, true);
  assert.deepEqual(selected.data.members.map((member) => member.duty),
    fixture.people.filter((member) => fixture.projects[0].member_person_ids.includes(member.person_id)).map((member) => member.role));
  const model = await canonical.call('GET /api/team/overview-model', '/api/team/overview-model?project=');
  assert.equal(model.ok, true, JSON.stringify(model));
  assert.equal(model.project, '');
  assert.equal(model.input.tasks.items.length, fixture.tasks.length);
  const connections = await canonical.call('POST /api/team/connections/list', '/api/team/connections/list', { limit: 50 });
  assert.equal(connections.ok, true, JSON.stringify(connections));
  assert.equal(connections.data.items.length, fixture.people.flatMap((member) => member.agents).length);

  const full = bridgeForTest({ demoTimestampShiftMs: 0, now: () => fixture.now_unix_ms });
  assert.equal((await full.call('POST /api/team/directory', '/api/team/directory', {})).error.code, 'Unsupported');
  const fullSelected = await full.call('POST /api/team/directory', '/api/team/directory', { project: 'P-001' });
  assert.equal(fullSelected.ok, true);
  assert.equal(fullSelected.data.members.some((member) => Object.hasOwn(member, 'duty')), false,
    'the full variant keeps its directory projection without the canonical-only duty field');
});

test('the organization connections read is canonical-only and the organization model carries every project\'s approvals', async () => {
  const full = bridgeForTest({ demoTimestampShiftMs: 0, now: () => fixture.now_unix_ms });
  assert.equal((await full.call('POST /api/team/connections/list', '/api/team/connections/list', { limit: 50 })).error.code,
    'Unsupported');
  const canonical = bridgeForTest({ demoVariant: 'canonical', demoTimestampShiftMs: 0, now: () => fixture.now_unix_ms });
  const model = await canonical.call('GET /api/team/overview-model', '/api/team/overview-model?project=');
  assert.equal(model.ok, true, JSON.stringify(model));
  const whole = canonicalOverviewInput(fixture);
  assert.ok(whole.approvals.items.length > 0, 'the canonical fixture records approvals');
  assert.equal(model.input.approvals.items.length, whole.approvals.items.length);
  assert.deepEqual([...new Set(model.input.approvals.items.map((item) => item.projectId))].sort(),
    [...new Set(whole.approvals.items.map((item) => item.projectId))].sort());
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

test('only the full demo publishes approvals and the demo viewer scope, and members carry no actor ids', async () => {
  const model = await overviewModelForTest({ demoTimestampShiftMs: 0, now: () => fixture.now_unix_ms + 9 * DAY });
  assert.deepEqual(model.input.options.nowUnixMs, fixture.now_unix_ms);
  assert.deepEqual(model.input.options.views.me, { personId: fixture.viewer.person_id, projectId: 'P-001' });
  assert.deepEqual(model.input.approvals, complete(fixture.approvals));
  assert.equal(model.coverage.approvals.status, 'complete');
  const member = model.input.members.items.find((item) => item.personId === 'M-002');
  assert.deepEqual(member, { personId: 'M-002', displayName: '林悦', groupId: 'G-001' });

  const liveMember = overviewMemberFromDirectory({
    member_ref: { project_id: 'synthetic-project', person_id: 'synthetic-person' },
    display_name: 'Synthetic Member', department_ids: ['synthetic-department'],
  });
  assert.deepEqual(liveMember, {
    personId: 'synthetic-person', displayName: 'Synthetic Member', groupId: 'synthetic-department',
  });
  // Audit attribution no longer travels through the directory: a reply that still sends
  // actor_ids is ignored rather than copied into the overview member shape.
  assert.deepEqual(overviewMemberFromDirectory({
    member_ref: { project_id: 'P-001', person_id: 'M-002' }, display_name: '林悦',
    department_ids: ['G-001'], actor_ids: ['M-002', 'M-002-agent'],
  }), { personId: 'M-002', displayName: '林悦', groupId: 'G-001' });

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

test('demo work.next identity has no person_id and Agent audit records expose their own disclosure', () => {
  const demo = createDemoOrganization({ variant: 'full', timestampShiftMs: 0 });
  const next = demo.query('P-001', { protocol_version: 1, op: 'work.next' });
  assert.equal(Object.hasOwn(next.data.identity, 'person_id'), false);
  assert.equal(next.data.identity.actor_id, fixture.viewer.actor_id);
  assert.equal(next.data.identity.client_id, fixture.viewer.client_id);
  assert.deepEqual(next.data.identity.business_roles, fixture.viewer.business_roles);

  const audit = ['P-001', 'P-002'].flatMap((project) =>
    demo.query(project, { protocol_version: 1, op: 'audit.development' }).data.items);
  const byId = new Map(audit.map((item) => [item.id, item]));
  // Every record carries the AWR-PPD-055 disclosure objects, whatever they hold.
  for (const item of audit) {
    assert.deepEqual(Object.keys(item.actor).sort(), ['display_name', 'kind', 'person_id']);
    assert.deepEqual(Object.keys(item.client), ['display_name']);
  }
  // A client-attributed run names the client and resolves the person behind it.
  assert.equal(byId.get('EV-001').client_id, 'M-002-claude');
  assert.deepEqual(byId.get('EV-001').actor, { person_id: 'M-002', display_name: '林悦', kind: 'agent' });
  assert.deepEqual(byId.get('EV-001').client, { display_name: 'Claude 桌面' });
  assert.equal(byId.get('EV-009').client_id, 'M-002-codex');
  assert.deepEqual(byId.get('EV-009').actor, { person_id: 'M-002', display_name: '林悦', kind: 'agent' });
  assert.deepEqual(byId.get('EV-009').client, { display_name: 'Codex' });
  assert.equal(byId.get('EV-014').client_id, 'M-006-codex');
  assert.deepEqual(byId.get('EV-014').actor, { person_id: 'M-006', display_name: '赵衡', kind: 'agent' });
  for (const item of audit) {
    if (item.client_id != null) assert.match(item.client_id, /^M-\d{3}-(?:claude|codex)$/);
  }
  // A person acting through a client keeps the client unnamed in the attribution.
  assert.equal(byId.get('EV-002').client_id, null);
  assert.deepEqual(byId.get('EV-002').actor, { person_id: 'M-001', display_name: '周宁', kind: 'human' });
  assert.deepEqual(byId.get('EV-002').client, { display_name: 'Claude 桌面' });
  assert.deepEqual(byId.get('EV-005').client, { display_name: 'Claude 桌面' });
  // A run with no named client discloses neither a client name nor an attribution.
  assert.equal(byId.get('EV-006').client_id, null);
  assert.deepEqual(byId.get('EV-006').actor, { person_id: 'M-004', display_name: '许宁', kind: 'agent' });
  assert.deepEqual(byId.get('EV-006').client, { display_name: null });

  const humanDemo = createDemoOrganization({ variant: 'full', timestampShiftMs: 0 });
  // A person acting through a client: the record says so itself (AWR-PPD-062).
  Object.assign(humanDemo.data.activity[0], {
    actor_id: 'M-002', actor_kind: 'human', actor_display_name: '林悦（通过 Claude 桌面）',
  });
  const human = humanDemo.query('P-001', { protocol_version: 1, op: 'audit.development' }).data.items[0];
  assert.equal(human.client_id, null, 'a person acting through a client is not attributed to it');
  assert.deepEqual(human.actor, { person_id: 'M-002', display_name: '林悦', kind: 'human' });
  assert.deepEqual(human.client, { display_name: 'Claude 桌面' });
});

// AWR-PPD-062: the page writes "client (person)" for an Agent's run and "person (via client)"
// for a person's, following the recorded actor kind. Every demo record states who acted, and
// composing that sentence from the reply gives exactly the prototype's own phrase.
test('demo audit records state who acted and still compose the prototype phrase', () => {
  const demo = createDemoOrganization({ variant: 'full', timestampShiftMs: 0 });
  const records = ['P-001', 'P-002', 'P-003'].flatMap((project) => ['audit.development', 'audit.requests']
    .flatMap((op) => demo.query(project, { protocol_version: 1, op }).data.items));
  assert.equal(records.length, fixture.activity.length * 2);
  for (const item of records) {
    const record = fixture.activity.find((entry) => entry.id === item.id);
    assert.ok(['agent', 'human'].includes(record.actor_kind), `${item.id} states who acted`);
    assert.equal(item.actor.kind, record.actor_kind, item.id);
    assert.equal(item.client.display_name != null, record.actor_display_name.includes('（'), item.id);
    if (item.client.display_name != null) {
      // An Agent's run is attributed to the client; a person's run through a client is not.
      assert.equal(item.actor.kind === 'agent', item.client_id != null, item.id);
    }
    const { display_name: person } = item.actor;
    const client = item.client.display_name;
    const sentence = client == null ? person
      : item.actor.kind === 'agent' ? `${client}（${person}）`
        : item.actor.kind === 'human' ? `${person}（通过 ${client}）` : person;
    assert.equal(sentence, record.actor_display_name, item.id);
    // The sentence the page wrote before AWR-PPD-062, by the presence of the client id,
    // is the same sentence: switching the rule to the recorded kind changes no row.
    const before = client == null ? person
      : item.client_id != null ? `${client}（${person}）` : `${person}（通过 ${client}）`;
    assert.equal(before, sentence, item.id);
  }
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

/** Monday-00:00 to Sunday-24:00 calendar week of a moment, in the viewer's own zone. */
function localWeekBounds(nowUnixMs) {
  const date = new Date(nowUnixMs);
  const midnight = new Date(date.getFullYear(), date.getMonth(), date.getDate()).getTime();
  const weekday = (date.getDay() + 6) % 7;
  const start = midnight - weekday * DAY;
  return [start, start + WEEK - 1];
}

test('whole-week demo clock offsets keep the week-window numbers stable', () => {
  // The page computes its "this week" window from the viewer's calendar, so the default
  // offset rounds to whole weeks: the fixture's Thursday noon stays Thursday noon and its
  // calendar weeks keep catching the same events. Relative time alone is preserved under
  // any offset (see the startup-offset test above); only whole weeks keep the week windows.
  const expected = {
    me: [2, 1, 2, 3], lead: [2, 7, 7, 2], top: [3, 12, 5, 0],
  };
  const week0 = deriveOverview(canonicalOverviewInput(
    createDemoOrganization({ variant: 'full', timestampShiftMs: 0 }).data));
  const previousWeek = { me: week0.views.me.metrics[0].previousPeriod.count,
    lead: week0.views.lead.metrics[0].previousPeriod.count };
  for (const weeks of [0, 1, 3, -2, 26, 400]) {
    const demo = createDemoOrganization({ variant: 'full', timestampShiftMs: weeks * WEEK });
    const result = deriveOverview(canonicalOverviewInput(demo.data));
    const counts = Object.fromEntries(Object.entries(result.views).map(([view, summary]) =>
      [view, summary.metrics.map((metric) => metric.count)]));
    assert.deepEqual({ me: counts.me, lead: counts.lead, top: counts.top }, expected, `offset ${weeks} weeks`);
    // Department keeps every scope-and-wait number; only its month metric is calendar-bound
    // (a month is not a whole number of weeks, so facts slide across month boundaries).
    assert.deepEqual([counts.department[0], counts.department[2], counts.department[3]], [3, 5, 0],
      `offset ${weeks} weeks: department scope, cross-group and pending numbers`);
    assert.equal(result.views.me.metrics[0].previousPeriod.count, previousWeek.me,
      `offset ${weeks} weeks: me previous week`);
    assert.equal(result.views.lead.metrics[0].previousPeriod.count, previousWeek.lead,
      `offset ${weeks} weeks: lead previous week`);
    assert.deepEqual(result.views.me.metrics[2].items.map((wait) => wait.task.id).sort(), ['T-103', 'T-111'],
      'the owner self-answer wait belongs to her Agent, not to the prototype waiting-on-others list');
    assert.ok(result.waits.every((wait) => wait.durationDays >= 0));
    const [start, end] = localWeekBounds(demo.data.now_unix_ms);
    const verifiedThisWeek = demo.data.tasks.filter((task) => statusOf(task) === 'verified'
      && task.verified_at_unix_ms >= start && task.verified_at_unix_ms <= end).length;
    assert.equal(verifiedThisWeek, 2, `the calendar week of "now" catches the same events after ${weeks} weeks`);
  }
  // The month metric drifts by design: one week of offset slides T-202's late-October
  // verification into November. Pinned so the drift stays a documented, reviewed number
  // instead of an invisible change to the department view's second figure.
  const drifted = deriveOverview(canonicalOverviewInput(
    createDemoOrganization({ variant: 'full', timestampShiftMs: WEEK }).data));
  assert.equal(drifted.views.department.metrics[1].count, 11);
});

test('the default demo clock offset is a whole number of weeks', () => {
  for (let attempt = 0; attempt < 3; attempt += 1) {
    const demo = createDemoOrganization({ variant: 'full' });
    const shift = demo.data.now_unix_ms - fixture.now_unix_ms;
    assert.ok(Number.isInteger(shift / WEEK),
      `the default offset must be whole weeks, got ${shift} ms (${shift / WEEK} weeks)`);
    assert.equal(new Date(demo.data.now_unix_ms).getUTCDay(), new Date(fixture.now_unix_ms).getUTCDay(),
      'the fixture "now" keeps its weekday');
    assert.equal(demo.data.now_unix_ms % DAY, fixture.now_unix_ms % DAY,
      'the fixture "now" keeps its time of day');
  }
  // An explicit offset still wins, including non-whole-week ones for relative-time tests.
  assert.equal(createDemoOrganization({ variant: 'full', timestampShiftMs: 1234 })
    .data.now_unix_ms, fixture.now_unix_ms + 1234);
});

test('a non-whole-week offset can move events across the calendar-week boundary', () => {
  // Only relative time is guaranteed under an arbitrary offset (the startup-offset test);
  // a part-week slide drags verified timestamps over the week boundary, which is exactly
  // why the default rounds to whole weeks. Pinned so a future "smarter" rounding that
  // drifts by days fails here instead of quietly rescheduling the prototype's weeks.
  const demo = createDemoOrganization({ variant: 'full', timestampShiftMs: 4 * DAY });
  const [start, end] = localWeekBounds(demo.data.now_unix_ms);
  const verifiedThisWeek = demo.data.tasks.filter((task) => statusOf(task) === 'verified'
    && task.verified_at_unix_ms >= start && task.verified_at_unix_ms <= end).length;
  assert.equal(verifiedThisWeek, 0,
    'four days of drift move every "verified this week" event out of the current week');
});

const PROTOTYPE_METRICS = {
  me: [2, 1, 2, 3], lead: [2, 7, 7, 2], department: [3, 10, 5, 0], top: [3, 12, 5, 0],
};

async function canonicalModelFor(project) {
  const { call } = bridgeForTest({ demoVariant: 'canonical', demoTimestampShiftMs: 0,
    now: () => fixture.now_unix_ms });
  const model = await call('GET /api/team/overview-model', `/api/team/overview-model?project=${project}`);
  assert.equal(model.ok, true, JSON.stringify(model));
  return model;
}

test('the canonical variant serves the fully-known overview model for each project', async () => {
  assert.equal(DEMO_VARIANTS.has('canonical'), true);
  const tasksByProject = Object.fromEntries(fixture.projects.map((project) => [project.project_id,
    fixture.tasks.filter((task) => task.project_id === project.project_id).length]));
  const byId = (items) => new Map(items.map((item) => [item.id, item]));
  const union = { tasks: [], dependencies: [], decisions: [], reviewRounds: [], handoffs: [],
    members: [], activity: [], approvals: [] };
  for (const project of ['P-001', 'P-002', 'P-003']) {
    const model = await canonicalModelFor(project);
    assert.equal(model.project, project);
    // Every plane is complete and honest about the whole project's facts.
    for (const [plane, detail] of Object.entries(model.coverage)) {
      assert.deepEqual(detail, { status: 'complete', missing: [] }, `${project} ${plane} coverage`);
      assert.equal(model.input[plane].coverage, 'complete', `${project} ${plane} slice`);
    }
    assert.equal(model.input.tasks.items.length, tasksByProject[project]);
    assert.deepEqual([...new Set(model.input.tasks.items.map((task) => task.projectId))], [project]);
    assert.ok(model.input.tasks.items.every((task) => task.externalWait !== undefined
      && task.verifiedAtUnixMs !== undefined && task.submittedResultAtUnixMs !== undefined
      && task.commitments !== undefined && task.acceptance !== undefined),
      `${project}: the facts the live reads cannot supply yet are all present`);
    assert.deepEqual(model.input.approvals.items.map((approval) => approval.id).sort(),
      fixture.approvals.filter((approval) => approval.projectId === project)
        .map((approval) => approval.id).sort(), `${project} approvals stay project-scoped`);
    // Options mirror the full path's fixed "now" and the viewer's me scope, plus the
    // calendar periods and stall threshold the canonical clock makes knowable; the
    // threshold must stay the canonical one or the prototype statuses stop matching.
    assert.deepEqual(model.input.options, {
      nowUnixMs: fixture.now_unix_ms,
      stalledAfterDays: 1,
      views: {
        me: { personId: fixture.viewer.person_id, projectId: project },
        currentWeek: canonicalOverviewInput(fixture).options.views.currentWeek,
        previousWeek: canonicalOverviewInput(fixture).options.views.previousWeek,
        currentMonth: canonicalOverviewInput(fixture).options.views.currentMonth,
        previousMonth: canonicalOverviewInput(fixture).options.views.previousMonth,
      },
    });
    assert.equal(model.freshness.authoritative.status, 'measured');
    assert.equal(model.freshness.authoritative.at_unix_ms, fixture.now_unix_ms);
    assert.equal(model.freshness.page_read.at_unix_ms, model.read_at_unix_ms);
    // Deriving one project's reply leaves every task status known (no "unknown" state,
    // the honest gap the full variant currently carries) and reproduces the me numbers.
    const derived = deriveOverview(model.input);
    assert.ok(derived.tasks.every((task) => task.status !== 'unknown'), `${project} task statuses`);
    assert.equal(derived.coverage.sections.status.complete, true, `${project} status section`);
    assert.equal(derived.coverage.sections.waits.complete, true, `${project} waits section`);
    if (project === 'P-001') {
      assert.deepEqual(derived.views.me.metrics.map((metric) => metric.count), PROTOTYPE_METRICS.me);
      const question = model.input.decisions.items.find((item) => item.id === 'Q-001');
      const pending = fixture.pending.find((item) => item.pending_id === 'A-001');
      assert.equal(question.question, '空状态要不要显示引导文案');
      assert.equal(question.description, pending.description);
      assert.match(question.description, /订单列表为空时/);
      // The report fact stays at the prototype's Nov 17 14:00; the page's calendar-day
      // rule turns its 47-hour interval into the prototype's two-day report age.
      assert.equal(model.input.tasks.items.find((task) => task.id === 'T-104').report.reportedAtUnixMs,
        Date.parse('2026-11-17T14:00:00.000Z'));
    }
    for (const plane of Object.keys(union)) {
      for (const item of model.input[plane].items) union[plane].push(item);
    }
  }
  // The three project inputs partition the whole canonical input: their union plus the
  // canonical view scopes reproduces every prototype number exactly.
  const whole = canonicalOverviewInput(fixture);
  for (const plane of Object.keys(union)) {
    assert.deepEqual([...byId(union[plane]).keys()].sort(), [...byId(whole[plane].items).keys()].sort(),
      `${plane}: the union of the three project inputs is the whole canonical input`);
    for (const [id, item] of byId(union[plane])) {
      assert.deepEqual(item, byId(whole[plane].items).get(id), `${plane} ${id} survives project scoping unchanged`);
    }
  }
  const merged = deriveOverview({ ...whole, tasks: whole.tasks,
    dependencies: whole.dependencies, decisions: whole.decisions,
    reviewRounds: whole.reviewRounds, handoffs: whole.handoffs,
    members: whole.members, activity: whole.activity, approvals: whole.approvals });
  assert.deepEqual(Object.fromEntries(Object.entries(merged.views).map(([view, summary]) =>
    [view, summary.metrics.map((metric) => metric.count)])), PROTOTYPE_METRICS);
});

test('the canonical project directory serves the prototype four rows with lead, group and takeover facts', async () => {
  const canonical = bridgeForTest({ demoVariant: 'canonical', demoTimestampShiftMs: 0,
    now: () => fixture.now_unix_ms });
  const reply = await canonical.call('GET /api/team/projects', '/api/team/projects');
  assert.equal(reply.ok, true);
  assert.equal(reply.view, 'team');
  assert.deepEqual(reply.projects.map((project) => project.key), ['P-001', 'P-002', 'P-003', 'P-004']);
  const byKey = new Map(reply.projects.map((project) => [project.key, project]));
  // The three ordinary rows carry the recorded lead (resolvable through that project's
  // own overview model member directory) and the group's display name, exactly the
  // prototype's 周宁/吴桐/郑岚 and their groups.
  const leads = { 'P-001': '周宁', 'P-002': '吴桐', 'P-003': '郑岚' };
  const groups = { 'P-001': '应用小组', 'P-002': '平台小组', 'P-003': '交付小组' };
  for (const key of ['P-001', 'P-002', 'P-003']) {
    const entry = byKey.get(key);
    assert.equal(entry.title, fixture.projects.find((project) => project.key === key).title, key);
    assert.equal(entry.takeover, null, `${key} is a recorded non-takeover project`);
    assert.ok(fixture.projects.find((project) => project.key === key).member_person_ids
      .includes(entry.leadPersonId), `${key}'s lead belongs to the project`);
    assert.equal(entry.groupName, groups[key], `${key}'s group display name`);
    const model = await canonical.call('GET /api/team/overview-model', `/api/team/overview-model?project=${key}`);
    const member = model.input.members.items.find((item) => item.personId === entry.leadPersonId);
    assert.equal(member.displayName, leads[key], `${key}'s lead display name`);
  }
  // The fourth row is the prototype's taken-over sample-cli with its deterministic
  // takeover facts; its recorded moment is the demo clock half a minute back, which is
  // what makes the page's update column read 刚刚.
  const takeover = byKey.get('P-004');
  assert.equal(takeover.title, 'sample-cli');
  assert.deepEqual(takeover.takeover, {
    resultId: 'TK-001',
    recordedAtUnixMs: fixture.now_unix_ms - 30_000,
    verifiedTasks: 0,
    workstreams: 4,
    questions: 4,
  });
  // ProjectsPage does not query the takeover model. The existing shell's inbox indicators
  // do query every directory key, so canonical gives that independent caller a scoped empty
  // known model rather than an authorization failure; keys outside the directory stay denied.
  const takeoverModel = await canonical.call('GET /api/team/overview-model',
    '/api/team/overview-model?project=P-004');
  assert.equal(takeoverModel.ok, true);
  assert.deepEqual(takeoverModel.input.tasks, { coverage: 'complete', items: [] });
  assert.deepEqual(takeoverModel.periodUpdates, { coverage: 'complete', items: [] });
  const unknownModel = await canonical.call('GET /api/team/overview-model',
    '/api/team/overview-model?project=P-999');
  assert.equal(unknownModel.ok, false);
  assert.equal(unknownModel.error.code, 'Forbidden');
  assert.equal(unknownModel.input, undefined, 'a denied composition exposes no facts');
  // The full variant keeps its plain three-entry directory byte for byte.
  const full = bridgeForTest({ demoTimestampShiftMs: 0, now: () => fixture.now_unix_ms });
  const fullReply = await full.call('GET /api/team/projects', '/api/team/projects');
  assert.deepEqual(fullReply.projects, fixture.projects.map((project) => ({
    key: project.key, title: project.title, role: fixture.viewer.role,
  })));
});

test('the canonical period updates keep the prototype weeks and ages on the demo clock', async () => {
  const bridge = bridgeForTest({ demoVariant: 'canonical', demoTimestampShiftMs: 0,
    now: () => fixture.now_unix_ms });
  const updatesByProject = {};
  const selectedUpdateIds = { 'P-001': ['U-001'], 'P-002': ['U-004'], 'P-003': ['U-007'] };
  for (const key of ['P-001', 'P-002', 'P-003']) {
    const model = await bridge.call('GET /api/team/overview-model', `/api/team/overview-model?project=${key}`);
    assert.equal(model.periodUpdates.coverage, 'complete', `${key} period-update coverage`);
    assert.deepEqual(model.periodUpdates.items.map((item) => item.id), selectedUpdateIds[key],
      `${key} serves exactly its selected current or earlier update`);
    for (const item of model.periodUpdates.items) {
      const source = fixture.updates.find((update) => update.update_id === item.id);
      assert.deepEqual({ id: item.id, status: item.status, summary: item.summary,
        recordedAtUnixMs: item.recordedAtUnixMs }, {
        id: source.update_id, status: source.status, summary: source.summary,
        recordedAtUnixMs: source.recorded_at_unix_ms,
      }, `${item.id} keeps its recorded sentence, status and time`);
      assert.match(item.startsOn, /^\d{4}-\d{2}-\d{2}$/, `${item.id} startsOn is an ISO date`);
      assert.equal(item.endsOn > item.startsOn, true, `${item.id} spans a week`);
    }
    updatesByProject[key] = model.periodUpdates.items;
  }
  // Week 47 covers the fixture's "today"; the current updates are recorded the same
  // day (P-001) and exactly three days back (P-002), matching the prototype's 今天 and
  // 3 天前.
  const monday = (unixMs) => {
    const date = new Date(unixMs);
    const day = Date.UTC(date.getUTCFullYear(), date.getUTCMonth(), date.getUTCDate());
    return day - (date.getUTCDay() + 6) % 7 * DAY;
  };
  const iso = (unixMs) => new Date(unixMs).toISOString().slice(0, 10);
  const today = iso(fixture.now_unix_ms);
  const currentMonday = monday(fixture.now_unix_ms);
  const orders = updatesByProject['P-001'].find((item) => item.id === 'U-001');
  assert.deepEqual([orders.startsOn, orders.endsOn],
    [iso(currentMonday), iso(currentMonday + WEEK - DAY)]);
  assert.equal(orders.startsOn <= today && today <= orders.endsOn, true, 'U-001 covers today');
  assert.equal(orders.status, 'risk');
  assert.equal(fixture.now_unix_ms - orders.recordedAtUnixMs < DAY, true, 'U-001 was recorded today');
  const notify = updatesByProject['P-002'].find((item) => item.id === 'U-004');
  assert.equal(notify.status, 'ok');
  assert.equal(notify.startsOn <= today && today <= notify.endsOn, true, 'U-004 covers today');
  assert.equal(fixture.now_unix_ms - notify.recordedAtUnixMs, 3 * DAY, 'U-004 was recorded three days back');
  // P-003 has no current-period update: its newest record is a full two weeks old, its
  // period ends before the current week, and the age is exactly the prototype's 12 days.
  const stale = updatesByProject['P-003'];
  assert.equal(stale.some((item) => item.startsOn <= today && today <= item.endsOn), false,
    'no P-003 update covers today');
  const latest = [...stale].sort((left, right) => right.endsOn.localeCompare(left.endsOn))[0];
  assert.equal(latest.id, 'U-007');
  assert.equal(latest.endsOn < today, true, 'U-007 ends before the current week');
  assert.equal(fixture.now_unix_ms - latest.recordedAtUnixMs, 12 * DAY,
    'U-007 was recorded exactly twelve days back');
  // Whole-week offsets move the derived periods with the clock: five weeks on, the
  // current update still covers the shifted "today" and the stale record keeps its
  // two-week distance. Absolute dates are never hardcoded on the server.
  const shift = 5 * WEEK;
  const shifted = bridgeForTest({ demoVariant: 'canonical', demoTimestampShiftMs: shift,
    now: () => fixture.now_unix_ms + shift });
  const shiftedModel = await shifted.call('GET /api/team/overview-model', '/api/team/overview-model?project=P-001');
  const shiftedCurrent = shiftedModel.periodUpdates.items.find((item) => item.id === 'U-001');
  const shiftedMonday = monday(fixture.now_unix_ms + shift);
  const shiftedToday = iso(fixture.now_unix_ms + shift);
  assert.deepEqual([shiftedCurrent.startsOn, shiftedCurrent.endsOn],
    [iso(shiftedMonday), iso(shiftedMonday + WEEK - DAY)]);
  assert.equal(shiftedCurrent.startsOn <= shiftedToday && shiftedToday <= shiftedCurrent.endsOn, true,
    'U-001 still covers the shifted today');
  const shiftedStaleModel = await shifted.call('GET /api/team/overview-model',
    '/api/team/overview-model?project=P-003');
  assert.equal(shiftedStaleModel.periodUpdates.items[0].id, 'U-007');
  assert.equal(shiftedStaleModel.periodUpdates.items[0].startsOn, iso(shiftedMonday - 2 * WEEK),
    'the selected earlier period keeps its week distance');
  // The full variant's reply carries no period-update slice at all; the snapshot test
  // below pins that byte for byte, this keeps the intent readable.
  const full = bridgeForTest({ demoTimestampShiftMs: 0, now: () => fixture.now_unix_ms });
  const fullModel = await full.call('GET /api/team/overview-model', '/api/team/overview-model?project=P-001');
  assert.equal('periodUpdates' in fullModel, false);
});

test('demo freshness reports the answer moment while the derivation clock stays on the fixture week', async () => {
  // The #709 review decision: input.options.nowUnixMs keeps the whole-week fixture
  // clock (so week windows and relative ages match the prototype), while the reply's
  // read time and every freshness measurement report the moment this server answered.
  // With zero offset the fixture clock is days away from the real present, so a
  // fixture-clock freshness reading would fail these windows.
  for (const variant of ['full', 'canonical']) {
    const before = Date.now();
    const bridge = bridgeForTest({ demoVariant: variant, demoTimestampShiftMs: 0 });
    const model = await bridge.call('GET /api/team/overview-model', '/api/team/overview-model?project=P-001');
    const after = Date.now();
    assert.equal(model.ok, true, JSON.stringify(model));
    assert.equal(model.input.options.nowUnixMs, fixture.now_unix_ms,
      `${variant}: the derivation clock stays on the fixture time`);
    assert.ok(model.read_at_unix_ms >= before - 1000 && model.read_at_unix_ms <= after + 1000,
      `${variant}: read_at_unix_ms ${model.read_at_unix_ms} is outside the call window [${before}, ${after}]`);
    assert.equal(model.freshness.page_read.status, 'measured', `${variant} page_read`);
    assert.equal(model.freshness.page_read.at_unix_ms, model.read_at_unix_ms, `${variant} page_read time`);
    assert.equal(model.freshness.authoritative.status, 'measured', `${variant} authoritative status`);
    assert.equal(model.freshness.authoritative.at_unix_ms, model.read_at_unix_ms,
      `${variant} authoritative measurement time`);
    const age = model.freshness.authoritative.age_ms;
    assert.ok(typeof age === 'number' && age >= 0 && age < 60_000,
      `${variant}: authoritative age ${age} is not the answer moment's age`);
  }
});

test('the full variant keeps the merged overview-model reply shape byte for byte', async () => {
  // Pins the #704 reply as merged: zero offset and a fixed clock make every field
  // deterministic, so any shape drift — a dropped field, a renamed gap reason, a new
  // default — fails here before it can reach a pixel comparison.
  const { call } = bridgeForTest({ demoTimestampShiftMs: 0, now: () => fixture.now_unix_ms });
  const model = await call('GET /api/team/overview-model', '/api/team/overview-model?project=P-001');
  assert.equal(model.ok, true, JSON.stringify(model));
  assert.deepEqual(model, require('./fixtures/overview-model-full-demo.json'));
});

test('canonical directory and shell facts establish all four perspectives without relaxing their rules', async () => {
  const demo = createDemoOrganization({ variant: 'canonical', timestampShiftMs: 0 });
  const directory = demo.directory('P-001').json.data;
  assert.deepEqual(directory.plan.departments.map(({ id, name, parent_id }) => ({ id, name, parent_id: parent_id ?? null })), [
    { id: 'G-004', name: '研发部', parent_id: null },
    { id: 'G-005', name: '客户交付部', parent_id: null },
    { id: 'G-001', name: '应用小组', parent_id: 'G-004' },
    { id: 'G-002', name: '平台小组', parent_id: 'G-004' },
    { id: 'G-003', name: '交付小组', parent_id: 'G-005' },
  ]);
  assert.notEqual(projectDirectory(directory), null, 'the real directory validator accepts the complete hierarchy');
  const capabilitiesSample = JSON.parse(
    require('node:fs').readFileSync(
      require('node:path').resolve(__dirname, '../../../crates/awr-server/tests/samples/capabilities-web-read.json'),
      'utf8',
    ),
  );
  const demoCapabilities = demo.query('P-001', { protocol_version: 1, op: 'capabilities' });
  assert.deepEqual(demoCapabilities.business_role_presets, capabilitiesSample.business_role_presets,
    'demo capabilities publish the verbatim AWR-PPD-061 business duty action ceilings');

  const bridge = bridgeForTest({ demoVariant: 'canonical', demoTimestampShiftMs: 0 });
  const reply = await bridge.call('GET /api/team/shell', '/api/team/shell');
  assert.deepEqual(reply, {
    ok: true,
    source: 'demo',
    organizationName: '示例组织',
    // The canonical fixture's clock times are the prototype's wall-clock times in UTC,
    // so this variant pins the pages' display zone; every other variant answers null.
    displayTimeZone: 'UTC',
    perspective: {
      supervisedGroupCount: 0,
      memberGroupProjectCount: 1,
      visibleProjectGroupCount: 3,
      visibleProjectDepartmentCount: 2,
      canViewAllProjects: true,
      directory: { groupsRegistered: true, departmentsRegistered: true },
      pending: { me: null, lead: null, dept: null, top: null },
    },
  });
  assert.equal(bridge.bridge.TEAM.demoOrganization.data.viewer.can_view_all_projects, true,
    'the authorization is an explicit recorded viewer-scope fact, not inferred from coverage');
});

test('shell source is a bridge fact while every pre-existing variant reply stays unchanged', async () => {
  for (const variant of DEMO_VARIANTS) {
    if (variant === 'canonical') continue;
    const bridge = bridgeForTest({ demoVariant: variant, demoTimestampShiftMs: 0, now: () => fixture.now_unix_ms });
    assert.deepEqual(await bridge.call('GET /api/team/shell', '/api/team/shell'),
      { ok: true, source: 'demo', organizationName: '示例组织', perspective: null, displayTimeZone: null }, variant);
    assert.notEqual(bridge.bridge.TEAM.demoOrganization.data.viewer.can_view_all_projects, true,
      `${variant}: only canonical records the viewer's all-projects grant`);
    const projects = await bridge.call('GET /api/team/projects', '/api/team/projects');
    assert.deepEqual(Object.keys(projects), ['ok', 'projects', 'session', 'view'], `${variant}: no new project-reply fields`);
    assert.equal('perspective' in projects, false, `${variant}: perspective facts use the separate shell read`);
    if (variant === 'full') {
      assert.deepEqual(projects.projects, fixture.projects.map((project) => ({
        key: project.key, title: project.title, role: fixture.viewer.role,
      })));
      const fullDirectory = bridge.bridge.TEAM.demoOrganization.directory('P-001').json.data;
      // Main's unchanged full slice: the project's group and parent plus the groups of its
      // own members (M-004 许宁 belongs to 平台小组), exactly as before this task.
      assert.deepEqual(fullDirectory.plan.departments.map(({ id, name, parent_id }) => ({ id, name, parent_id: parent_id ?? null })), [
        { id: 'G-004', name: '研发部', parent_id: null },
        { id: 'G-001', name: '应用小组', parent_id: 'G-004' },
        { id: 'G-002', name: '平台小组', parent_id: 'G-004' },
      ]);
    }
  }
  const live = bridgeForTest({ teamUrl: 'http://127.0.0.1:1', demo: false });
  assert.deepEqual(await live.call('GET /api/team/shell', '/api/team/shell'),
    { ok: true, source: 'live', perspective: null, organizationName: null, displayTimeZone: null });
});

test('the shell read is refused when the deployment is neither live nor a demo', async () => {
  const bridge = bridgeForTest({ demo: false });
  assert.deepEqual(await bridge.call('GET /api/team/shell', '/api/team/shell'),
    { ok: false, error: { code: 'DemoDisabled', message: 'fixtures require demo mode' } });
});
