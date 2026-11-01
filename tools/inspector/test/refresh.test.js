"use strict";
const { test, beforeEach } = require("node:test");
const assert = require("node:assert/strict");
require("./fixtures/dom-stub.js").install();
require("../public/demo-data.js");
const app = require("../public/app.js");
const $ = id => document.getElementById(id);
const json = data => ({json: async () => data});
const settle = () => new Promise(resolve => setImmediate(resolve));

function liveFetch(project = "project-a", items = ["B1", "B2"]) {
  return async url => {
    if (url === "/api/health") return json({ok: true, data: {mode: "live", project}});
    if (url === "/api/sources") return json({ok: true, data: {files: []}});
    if (url.startsWith("/api/work?")) {
      const key = new URL(url, "http://local").searchParams.get("key");
      return json({ok: true, data: {work: {external_key: key, title: `${project}: ${key}`}}});
    }
    if (url.startsWith("/api/assess")) {
      const key = new URL(url, "http://local").searchParams.get("key");
      return json({ok: true, data: {work: key, decision: {mode: "undetermined", reasons: [], unknown_observations: [], required_actions: [], optional_maintenance: [], completion_policy: "unchanged_source_policy", execution_admission: "not_granted_by_management_classification"}, admission_gaps: [], next_action: "record_current_assessment_with_explicit_host_observations"}});
    }
    if (url.startsWith("/api/spec-check")) {
      return json({ok: true, data: {status: "unknown", specifications: [], references: [], requirements: [], counts: {covered: 0, uncovered: 0, not_required: 0, needs_review: 0, unknown: 0}, diagnostics: ["no_explicit_specifications"], association_diagnostics: []}});
    }
    const data = {project_revision: 7, current_total: 0, ready_count: 0, waiting_count: 0,
      blocked_count: items.length, current: [], ready: [], waiting: [], blocked: items.map(key => ({key, title: key})), omissions: {}};
    if (url.startsWith("/api/work-page")) {
      const query = new URL(url, "http://local").searchParams;
      const offset = Number(query.get("offset")), limit = Number(query.get("limit"));
      data.page = {queue: query.get("queue"), offset, limit, total: items.length, has_more: offset + limit < items.length,
        items: data.blocked.slice(offset, offset + limit).map(w => ({...w, queue: "blocked"}))};
    }
    return json({ok: true, data});
  };
}
function compileButton() {
  return $("workDetail").find(el => el.tagName === "BUTTON" && el.textContent.includes("Compile context for this item"));
}
async function selectOverview() {
  await $("queueList").children[0].click();
  await compileButton().click();
}
beforeEach(async () => {
  Object.assign(app.state, {mode: "demo", project: "before-test", queueTab: "blocked", workFilter: "all",
    overviewWork: null, selectedWork: null, status: null, sources: null, workDetail: {}, workAssess: {}, specCheck: {}, raw: {}, compile: null,
    workPageSize: 10, workOffset: 0, workPage: null, workPageLoading: false, workPageError: null});
  $("fWork").textContent = "";
  $("fBudget").value = "6000";
  global.fetch = liveFetch();
  await app.loadAll();
  await selectOverview();
  assert.equal(app.state.overviewWork, "B1");
  assert.equal($("fWork").value, "B1");
});

for (const health of ["unreachable", "demo"]) {
  test(`switching live to ${health} clears task selection and context from the old source`, async () => {
    $("fGoal").value = "goal#old";
    $("fIntent").value = "old task intent";
    app.state.compile = {rendered: "old packet", sections: [], dimensions: [], evidenceGaps: [], unresolvedDeps: [], issues: [], omissions: []};
    app.state.raw.context = {old: true};
    global.fetch = async () => json(health === "demo"
      ? {ok: true, data: {mode: "demo"}}
      : {ok: false, error: {code: "BridgeUnreachable", message: "offline"}});
    await app.loadAll();
    assert.equal(app.state.project, ".local/demo");
    assert.equal(app.state.overviewWork, null);
    assert.equal($("detailId").textContent, app.state.raw.work.data.work.key);
    assert.ok($("detailId").textContent.startsWith("EXAMPLE-"));
    assert.ok(!$("fWork").children.some(option => option.value === "B1"));
    assert.equal($("fGoal").value, "");
    assert.equal($("fIntent").value, "");
    assert.equal(app.state.compile, null);
    assert.equal(app.state.raw.context, undefined);
    assert.equal($("rawContextBody").textContent, "");
    assert.equal($("packetPreview").textContent, "");
    await compileButton().click();
    assert.equal($("fWork").value, $("detailId").textContent);
  });
}

test("recovering from demo loads a real task without carrying the demo target back", async () => {
  global.fetch = async () => json({ok: false});
  await app.loadAll();
  await selectOverview();
  assert.ok(app.state.overviewWork.startsWith("EXAMPLE-"));
  global.fetch = liveFetch();
  await app.loadAll();
  assert.equal(app.state.mode, "live");
  assert.equal(app.state.overviewWork, null);
  assert.equal($("detailId").textContent, "B1");
  assert.equal($("fWork").value, "B1");
  assert.ok(!$("fWork").children.some(option => option.value.startsWith("EXAMPLE-")));
});

test("switching live projects resets the page and does not reuse a same-key detail", async () => {
  app.state.workOffset = 10;
  global.fetch = liveFetch("project-b", ["B1", "B9"]);
  await app.loadAll();
  assert.equal(app.state.project, "project-b");
  assert.equal(app.state.overviewWork, null);
  assert.equal(app.state.workOffset, 0);
  assert.equal($("detailId").textContent, "B1");
  assert.match($("workDetail").textContent, /project-b: B1/);
  assert.ok(!$("workDetail").textContent.includes("project-a"));
  assert.ok(!$("fWork").children.some(option => option.value === "B2"));
});

test("refresh within the same project preserves the explicit task even outside its queue", async () => {
  global.fetch = liveFetch("project-a", ["B2"]);
  await app.loadAll();
  assert.equal(app.state.overviewWork, "B1");
  assert.equal($("detailId").textContent, "B1");
  assert.match($("workDetail").textContent, /project-a: B1/);
  assert.equal($("fWork").value, "B1");
});

test("late detail responses from the old source do not enter the new source", async () => {
  let finish;
  global.fetch = () => new Promise(resolve => { finish = () => resolve(json({ok: true, data: {work: {external_key: "OLD", title: "old detail"}}})); });
  const old = app.renderWorkDetail("OLD");
  global.fetch = async () => json({ok: false});
  await app.loadAll();
  finish();
  await old;
  assert.ok(app.state.raw.work.data.work.key.startsWith("EXAMPLE-"));
  assert.ok(!$("workDetail").textContent.includes("old detail"));
});

test("late compilation from the old source cannot replace a new source's result", async () => {
  let finish;
  global.fetch = () => new Promise(resolve => { finish = () => resolve(json({ok: false, error: {code: "OldFailure", message: "old compilation"}})); });
  const old = app.doCompile();
  global.fetch = async () => json({ok: false});
  await app.loadAll();
  assert.equal($("compileBtn").disabled, false);
  await app.doCompile();
  const current = app.state.raw.context;
  finish();
  await old;
  assert.equal(app.state.raw.context, current);
  assert.ok(!$("breakdown").textContent.includes("old compilation"));
});

for (const delayed of ["health", "status"]) {
  test(`late ${delayed} from an older refresh cannot restore the previous project`, async () => {
    let finish;
    const previousFetch = liveFetch();
    global.fetch = url => url === `/api/${delayed}`
      ? new Promise(resolve => { finish = async () => resolve(await previousFetch(url)); })
      : previousFetch(url);
    const old = app.loadAll();
    await settle();
    assert.equal(typeof finish, "function");
    global.fetch = liveFetch("project-b", ["NEW"]);
    await app.loadAll();
    await finish();
    await old;
    assert.equal(app.state.project, "project-b");
    assert.equal($("detailId").textContent, "NEW");
    assert.equal($("fWork").value, "NEW");
    assert.equal(app.state.status.works[0].key, "NEW");
  });
}
