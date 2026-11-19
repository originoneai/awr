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
    // The fixture's pending approvals are not mapped into canonical facts yet; the demo-data
    // line owns that mapping. An empty complete read states these canonical facts establish
    // no approvals, which keeps the pending counts below honest for this oracle.
    approvals: complete([]),
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
  }, { projects: 3, people: 8, tasks: 29, documents: 9, updates: 8, goals: 3, lessons: 4, pending: 7 });
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
  assert.deepEqual(metrics.me, [{ count: 2, details: 2 }, { count: 1, details: 1 }, { count: 3, details: 3 }, { count: 2, details: 2 }]);
  assert.deepEqual(metrics.lead, [{ count: 2, details: 2 }, { count: 7, details: 7 }, { count: 7, details: 7 }, { count: 1, details: 1 }]);
  assert.deepEqual(metrics.department, [{ count: 3, details: 3 }, { count: 10, details: 10 }, { count: 5, details: 5 }, { count: 0, details: 0 }]);
  assert.deepEqual(metrics.top, [{ count: 3, details: 3 }, { count: 12, details: 12 }, { count: 5, details: 5 }, { count: 0, details: 0 }]);
  assert.equal(result.views.lead.metrics[0].previousPeriod.count, 2);
  assert.equal(result.views.department.metrics[1].previousPeriod.count, 2);
  assert.equal(result.views.top.metrics[1].claimed.count, 2);
  assert.equal(fixture.people.length, 8);
  assert.equal(fixture.view_roles.length, 2);
  assert.equal(result.peopleCounts.all.count, 10);

  // Prototype comparison: its primary-wait projection still differs from PPO-010's
  // per-edge/per-decision projection (the third and fourth numbers). The running-Agent
  // metric now agrees: a waiting task whose Agent is explicitly active counts for its
  // Agent (T-103), while the stalled T-104 and the inactive waiting T-111 do not.
  assert.deepEqual({
    me: { prototype: [2, 1, 3, 3], derived: metrics.me.map((metric) => metric.count) },
    lead: { prototype: [2, 7, 7, 2], derived: metrics.lead.map((metric) => metric.count) },
    department: { prototype: [3, 10, 2, 1], derived: metrics.department.map((metric) => metric.count) },
    top: { prototype: [3, 12, 2, 1], derived: metrics.top.map((metric) => metric.count) },
  }, {
    me: { prototype: [2, 1, 3, 3], derived: [2, 1, 3, 2] },
    lead: { prototype: [2, 7, 7, 2], derived: [2, 7, 7, 1] },
    department: { prototype: [3, 10, 2, 1], derived: [3, 10, 5, 0] },
    top: { prototype: [3, 12, 2, 1], derived: [3, 12, 5, 0] },
  });
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
