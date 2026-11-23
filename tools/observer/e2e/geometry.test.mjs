import assert from 'node:assert/strict';
import test from 'node:test';
import {
  buildPageRegistry,
  loadPageModules,
  registeredPageModules,
} from './demo-organization-check.mjs';
import activityModule, { checkActivityGeometry } from './geometry/activity.mjs';
import inboxModule, { checkInboxGeometry } from './geometry/inbox.mjs';
import membersModule, { checkMembersGeometry } from './geometry/members.mjs';
import projectsModule, { checkProjectsGeometry } from './geometry/projects.mjs';
import { checkWorkbenchGeometry } from './geometry/workbench.mjs';

function validActivityMetric(compact = false) {
  return {
    header: { paddingTop: compact ? '16px' : '24px', paddingBottom: compact ? '12px' : '16px' },
    filters: { gap: '12px', marginBottom: '16px' },
    segments: [{ gap: '2px', paddingTop: '2px', borderRadius: '8px' }],
    options: [{ height: 28, fontSize: '13px', paddingLeft: '10px', paddingRight: '10px', borderRadius: '6px' }],
    section: { borderRadius: '16px' },
    sectionHead: { height: 48, paddingLeft: '20px' },
    sectionTitle: { fontSize: '14px', lineHeight: '20px' },
    tableHead: { height: 40, fontSize: '12px' },
    firstTableHead: { paddingLeft: compact ? '16px' : '20px' },
    taskLink: { color: 'rgb(17, 24, 39)', textDecorationLine: 'none' },
    note: { paddingTop: '12px', paddingLeft: '20px', fontSize: '12px', lineHeight: '16px' },
    count: '2',
    rows: 2,
    rowHeights: [44, 46],
    times: ['今天 09:30', '昨天 16:40'],
    headers: compact ? ['什么时候', '谁', '做了什么'] : ['什么时候', '谁', '做了什么', '哪件任务'],
  };
}

function validInboxMetric(compact = false) {
  return {
    head: { paddingTop: compact ? '16px' : '24px', paddingBottom: compact ? '12px' : '16px' },
    lead: { fontSize: compact ? '18px' : '20px', lineHeight: compact ? '26px' : '28px', marginBottom: '20px' },
    md: { display: 'grid', columns: compact ? '358px' : '372px 748px', gap: '24px' },
    queue: { borderRadius: '16px' },
    firstRow: { paddingTop: '14px', paddingLeft: '16px', gap: '12px' },
    firstRowTitle: { fontWeight: '500' },
    firstRowMeta: { fontSize: '12px', lineHeight: '16px' },
    firstRowAge: { fontSize: '12px' },
    kindIcon: { width: 32, height: 32, radius: '8px' },
    focus: { borderRadius: '16px' },
    focusBody: { paddingTop: compact ? '20px' : '24px', paddingLeft: compact ? '20px' : '28px' },
    focusTitle: { fontSize: '22px', lineHeight: '28px' },
    focusFrom: { fontSize: '13px' },
    ask: { fontSize: '15px', lineHeight: '24px', marginTop: '20px' },
    basisHeading: { fontSize: '12px', marginBottom: '8px' },
    basisItem: { height: 36, paddingLeft: '12px', borderRadius: '8px' },
    where: { paddingTop: '16px', paddingLeft: compact ? '16px' : '28px', paddingBottom: compact ? '16px' : '20px' },
    whereHead: { fontWeight: '500' },
    sayline: { paddingLeft: '12px', borderRadius: '10px' },
    hint: { fontSize: '12px' },
    queueRows: 3,
    navCount: compact ? null : '3',
    bellLabel: compact ? null : '3 件待处理',
    bellDot: compact ? 0 : 1,
  };
}

function validProjectsMetric(compact = false) {
  return {
    forbidden: false,
    header: { paddingTop: compact ? '16px' : '24px', paddingBottom: compact ? '12px' : '16px' },
    lead: { marginBottom: '20px', fontSize: compact ? '18px' : '20px', lineHeight: compact ? '26px' : '28px', fontWeight: '500' },
    section: { borderRadius: '16px' },
    sectionHead: { height: 48, paddingLeft: '20px' },
    sectionTitle: { fontSize: '14px', lineHeight: '20px' },
    tableHead: { height: 40, fontSize: '12px' },
    firstTableHead: { paddingLeft: compact ? '16px' : '20px' },
    completionUnknown: { fontSize: '12px', lineHeight: '16px' },
    count: '3',
    rows: 3,
    headers: compact ? ['项目', '状态', '一句话', '完成得怎么样'] : ['项目', '状态', '一句话', '完成得怎么样', '更新'],
    freshness: true,
  };
}

function validMembersMetric(compact = false) {
  return {
    header: { paddingTop: compact ? '16px' : '24px', paddingBottom: compact ? '12px' : '16px' },
    lead: { fontSize: compact ? '18px' : '20px', lineHeight: compact ? '26px' : '28px', fontWeight: '500', marginBottom: '20px' },
    section: { borderRadius: '16px' },
    sectionHead: { height: 48, paddingLeft: '20px' },
    sectionTitle: { fontSize: '14px', lineHeight: '20px' },
    tableHead: { height: 40, fontSize: '12px' },
    firstTableHead: { paddingLeft: compact ? '16px' : '20px' },
    avatar: { width: 28, height: 28 },
    who: { gap: '10px' },
    chip: { height: 22, fontSize: '12px', paddingLeft: '8px', gap: '6px', borderRadius: '6px' },
    navigation: { width: 36 },
    note: { paddingTop: '12px', paddingLeft: '20px', fontSize: '12px', lineHeight: '16px' },
    borderCollapse: 'collapse',
    fixedArrows: 0,
    count: '8',
    rows: 8,
    headers: compact
      ? ['成员', '小组 · 职责', '现在在做', '等本人处理的']
      : ['成员', '小组 · 职责', '在哪些项目', '现在在做', 'Agent 连接', '等本人处理的'],
  };
}

test('registered page modules load cleanly and preserve report and canonical ordering', async () => {
  const modules = await loadPageModules(registeredPageModules);
  const registry = buildPageRegistry(modules);

  assert.deepEqual(
    registry.geometryModules.map((mod) => mod.reportKey),
    ['activityGeometry', 'inboxGeometry', 'projectsGeometry', 'membersGeometry'],
  );
  assert.deepEqual(
    registry.canonicalModules.map((mod) => mod.canonical.reportKey),
    ['canonicalWorkbenchRecords', 'canonicalProjectsRecords', 'canonicalMembersRecords'],
  );
  assert.deepEqual(
    [...registry.routeActions.keys()],
    ['workbench', 'project-plan', 'project-acceptance', 'project-knowledge'],
  );
});

test('a newly registered page module plugs into geometry, route actions and canonical without runner edits', async () => {
  const modules = await loadPageModules(registeredPageModules);
  const synthetic = {
    name: 'synthetic-page',
    reportKey: 'syntheticGeometry',
    matches: (route) => route.name === 'synthetic',
    measure: async () => ({ height: 48 }),
    check: (metric) => (metric.height === 48 ? [] : ['synthetic height drifted']),
    routeActions: { synthetic: async () => {} },
    canonical: {
      reportKey: 'canonicalSyntheticRecords',
      route: { name: 'synthetic-canonical', path: '/synthetic', run: async () => {} },
    },
  };
  const registry = buildPageRegistry([...modules, synthetic]);

  assert.equal(registry.geometryModules.at(-1)?.reportKey, 'syntheticGeometry');
  assert.equal(registry.canonicalModules.at(-1)?.canonical.reportKey, 'canonicalSyntheticRecords');
  assert.equal(registry.routeActions.has('synthetic'), true);
});

test('buildPageRegistry rejects duplicate module names, report keys, route actions and canonical keys', () => {
  assert.throws(() => buildPageRegistry([activityModule, activityModule]), /duplicate page module name/);
  assert.throws(() => buildPageRegistry([
    activityModule,
    { ...activityModule, name: 'activity-dup' },
  ]), /duplicate geometry reportKey/);
  assert.throws(() => buildPageRegistry([
    { name: 'a', routeActions: { shared: async () => {} } },
    { name: 'b', routeActions: { shared: async () => {} } },
  ]), /duplicate route action/);
  assert.throws(() => buildPageRegistry([
    { name: 'a', canonical: { reportKey: 'same', route: { name: 'r1', path: '/1', run: async () => {} } } },
    { name: 'b', canonical: { reportKey: 'same', route: { name: 'r2', path: '/2', run: async () => {} } } },
  ]), /duplicate canonical reportKey/);
});

test('route matchers target the exact routes from the monolithic script', () => {
  assert.equal(activityModule.matches({ name: 'activity-development' }), true);
  assert.equal(activityModule.matches({ name: 'activity-requests' }), true);
  assert.equal(activityModule.matches({ name: 'activity-partial' }), true);
  assert.equal(activityModule.matches({ name: 'inbox' }), false);

  assert.equal(inboxModule.matches({ name: 'inbox' }), true);
  assert.equal(inboxModule.matches({ name: 'inbox-canonical' }), false);

  assert.equal(projectsModule.matches({ name: 'projects' }), true);
  assert.equal(projectsModule.matches({ name: 'projects-canonical' }), false);
  assert.equal(projectsModule.matches({ name: 'project-overview' }), false);

  assert.equal(membersModule.matches({ name: 'members' }), true);
  assert.equal(membersModule.matches({ name: 'members-chooser' }), false);
  assert.equal(membersModule.matches({ name: 'members-canonical' }), false);
});

test('workbench geometry check passes on valid metrics and fails on 1px or layout drift', () => {
  const desktop = {
    titleSize: '22px',
    titleLeading: '28px',
    panelRadius: '16px',
    bodyColumns: 2,
    railWidth: 348,
    overflow: false,
  };
  checkWorkbenchGeometry(desktop, 1440);
  checkWorkbenchGeometry({ ...desktop, bodyColumns: 1, railWidth: 358 }, 390);

  assert.throws(() => checkWorkbenchGeometry({ ...desktop, railWidth: 349 }, 1440));
  assert.throws(() => checkWorkbenchGeometry({ ...desktop, titleSize: '23px' }, 1440));
  assert.throws(() => checkWorkbenchGeometry({ ...desktop, bodyColumns: 1 }, 1440));
  assert.throws(() => checkWorkbenchGeometry({ ...desktop, bodyColumns: 2 }, 390));
  assert.throws(() => checkWorkbenchGeometry({ ...desktop, overflow: true }, 1440));
});

test('activity geometry check passes on valid metrics and catches 1-unit mutations', () => {
  assert.deepEqual(checkActivityGeometry(validActivityMetric(false), false), []);
  assert.deepEqual(checkActivityGeometry(validActivityMetric(true), true), []);

  const badHead = validActivityMetric(false);
  badHead.sectionHead.height = 49;
  assert.match(checkActivityGeometry(badHead, false)[0], /section heading height: expected 48, got 49/);

  const badTime = validActivityMetric(false);
  badTime.times = ['今天 09:30:12'];
  assert.match(checkActivityGeometry(badTime, false)[0], /seconds remain/);

  const badRow = validActivityMetric(false);
  badRow.rowHeights = [47];
  assert.match(checkActivityGeometry(badRow, false)[0], /activity rows exceed 46px/);

  const badLink = validActivityMetric(false);
  badLink.taskLink.color = 'rgb(0, 0, 0)';
  assert.match(checkActivityGeometry(badLink, false)[0], /task link colour/);
});

test('inbox geometry check passes on valid metrics and catches 1-unit mutations', () => {
  assert.deepEqual(checkInboxGeometry(validInboxMetric(false), false), []);
  assert.deepEqual(checkInboxGeometry(validInboxMetric(true), true), []);

  const emptyDesktop = { ...validInboxMetric(false), queueRows: 0, navCount: null, bellDot: 0 };
  assert.deepEqual(checkInboxGeometry(emptyDesktop, false), []);

  const badGap = validInboxMetric(false);
  badGap.md.gap = '25px';
  assert.match(checkInboxGeometry(badGap, false)[0], /inbox grid gap: expected 24, got 25px/);

  const badPill = validInboxMetric(false);
  badPill.navCount = '2';
  assert.match(checkInboxGeometry(badPill, false)[0], /navigation count 2 does not match 3 queue rows/);

  const badCompactPill = validInboxMetric(true);
  badCompactPill.navCount = '3';
  assert.match(checkInboxGeometry(badCompactPill, true)[0], /compact inbox shows the navigation pill 3/);
});

test('projects geometry check passes on valid metrics and catches 1-unit mutations', () => {
  assert.deepEqual(checkProjectsGeometry(validProjectsMetric(false), false), []);
  assert.deepEqual(checkProjectsGeometry(validProjectsMetric(true), true), []);
  assert.deepEqual(checkProjectsGeometry({ forbidden: true }, false), []);

  const badTableHead = validProjectsMetric(false);
  badTableHead.tableHead.height = 41;
  assert.match(checkProjectsGeometry(badTableHead, false)[0], /project table heading height: expected 40, got 41/);

  const noFreshness = validProjectsMetric(false);
  noFreshness.freshness = false;
  assert.match(checkProjectsGeometry(noFreshness, false)[0], /freshness badge is missing/);
});

test('members geometry check passes on valid metrics and catches 1-unit mutations', () => {
  assert.deepEqual(checkMembersGeometry(validMembersMetric(false), false), []);
  assert.deepEqual(checkMembersGeometry(validMembersMetric(true), true), []);

  const badArrow = validMembersMetric(false);
  badArrow.navigation.width = 37;
  assert.match(checkMembersGeometry(badArrow, false)[0], /members navigation column width: expected 36, got 37/);

  const badBorder = validMembersMetric(false);
  badBorder.borderCollapse = 'separate';
  assert.match(checkMembersGeometry(badBorder, false)[0], /members table border model: separate/);

  const badFixed = validMembersMetric(false);
  badFixed.fixedArrows = 1;
  assert.match(checkMembersGeometry(badFixed, false)[0], /fixed cells: 1/);
});
