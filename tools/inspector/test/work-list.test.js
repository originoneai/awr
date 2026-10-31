"use strict";
const { test } = require("node:test");
const assert = require("node:assert/strict");
require("./fixtures/dom-stub.js").install();
const app = require("../public/app.js");
global.fetch = async () => ({ json: async () => ({ ok: false, error: { code: "fixture", message: "fixture" } }) });
function fixture(total = 17, returned = total, revision = 7) {
  const ready = Array.from({ length: total }, (_, i) => ({ key: `R${i}`, title: `Ready ${i}` }));
  return [{ project_revision: 7, ready_count: total, current_total: 2, blocked_count: 1, waiting_count: 0,
    current: [{key:"C1"}, {key:"C2"}], ready: ready.slice(0,5), waiting: [], blocked: [{key:"B1"}],
    omissions: {ready: Math.max(0,total-5)} },
    { project_revision: revision, ready_total: total, ready: ready.slice(0,returned), blocked_sample: [{key:"DO-NOT-MERGE"}] }];
}
function render(status, filter = "all") {
  app.state.mode = "live"; app.state.status = status; app.state.workFilter = filter; app.state.selectedWork = null;
  app.state.workDetail = {}; app.state.workAssess = {}; app.state.specCheck = {}; app.renderWork();
}
test("17 ready items replace the five-item summary and render all twenty queue items", () => {
  const status = app.normStatus(...fixture()); render(status);
  assert.equal(status.queues.ready.omitted, 0);
  assert.equal(document.getElementById("workRows").children.length, 20);
  assert.match(document.getElementById("workFilters").textContent, /Current queues 20/);
  render(status, "ready");
  assert.equal(document.getElementById("workRows").children.length, 17);
  assert.equal(document.getElementById("workSub").textContent, "17 items");
  assert.equal(status.queues.blocked.items[0].key, "B1");
});
test("bounded lists disclose missing rows instead of calling them complete", () => {
  const status = app.normStatus(...fixture(117,100)); render(status, "ready");
  assert.equal(status.queues.ready.total, 117);
  assert.equal(document.getElementById("workRows").children.length, 100);
  assert.match(document.getElementById("workSub").textContent, /17 more not loaded/);
  assert.match(document.getElementById("workEmpty").textContent, /not fully loaded/);
});
test("different revisions never merge queue membership from another snapshot", () => {
  const status = app.normStatus(...fixture(17,17,8)); render(status, "ready");
  assert.equal(status.queues.ready.items.length, 5);
  assert.match(document.getElementById("workSub").textContent, /12 more not loaded/);
});
test("failed ready request preserves summary and discloses omitted rows", () => {
  const status = app.normStatus(fixture()[0], null); render(status, "ready");
  assert.equal(status.queues.ready.items.length, 5);
  assert.equal(status.queues.ready.total, 17);
  assert.match(document.getElementById("workSub").textContent, /12 more not loaded/);
});

function pageResponse(offset, total = 17, queue = "ready") {
  return { ok: true, data: { ...fixture(total)[0], page: {queue, offset, limit:10, total,
    has_more:offset+10<total, items:Array.from({length:Math.max(0,Math.min(10,total-offset))},(_,i)=>({key:`R${offset+i}`,queue}))} } };
}
function pagination() {
  app.state.mode="live"; app.state.workPagination=true; app.state.workFilter="ready";
  app.state.workPageSize=10; app.state.workOffset=0; app.state.status=app.normStatus(...fixture());
  app.state.workPage=null; app.state.workPageLoading=false; app.state.workPageError=null;
  app.state.selectedWork=null; app.state.overviewWork=null; app.state.workDetail={};
}
test("server pages show ten then seven rows, not a slice of the summary", async () => {
  pagination(); const urls=[];
  global.fetch=async url=>({json:async()=>{urls.push(url);return pageResponse(Number(new URL(url,"http://local").searchParams.get("offset")));}});
  await app.loadWorkPage();
  assert.equal(document.getElementById("workRows").children.length,10);
  assert.match(document.getElementById("workPagination").textContent,/Page 1 \/ 2, 17 items/);
  app.state.workOffset=10; await app.loadWorkPage();
  assert.equal(document.getElementById("workRows").children.length,7);
  assert.equal(app.state.workPage.items[0].key,"R10");
  assert.ok(urls.some(url=>url.includes("offset=10&limit=10")));
});
test("late page responses cannot overwrite a newly selected queue", async () => {
  pagination(); let resolveFirst;
  global.fetch=url=>url.startsWith("/api/work-page") ? new Promise(resolve=>{resolveFirst=resolve;}) : Promise.resolve({json:async()=>({ok:false,error:{code:"fixture",message:"fixture"}})});
  const first=app.loadWorkPage();
  app.state.workFilter="blocked";
  global.fetch=async()=>({json:async()=>pageResponse(0,1,"blocked")});
  await app.loadWorkPage();
  resolveFirst({json:async()=>pageResponse(0)}); await first;
  assert.equal(app.state.workPage.queue,"blocked");
  assert.equal(document.getElementById("workRows").children.length,1);
});
test("removed last page moves back to the last available page", async () => {
  pagination(); app.state.workOffset=20;
  global.fetch=async url=>({json:async()=>pageResponse(Number(new URL(url,"http://local").searchParams.get("offset")),17)});
  await app.loadWorkPage(); assert.equal(app.state.workOffset,10);
  assert.equal(document.getElementById("workRows").children.length,7);
});
test("page failure clears old rows and does not pretend the queue is empty", async () => {
  pagination(); global.fetch=async()=>({json:async()=>({ok:false,error:{code:"Offline",message:"Disconnected"}})});
  await app.loadWorkPage(); assert.equal(app.state.workPage,null);
  assert.equal(document.getElementById("workRows").children.length,0);
  assert.match(document.getElementById("workEmpty").textContent,/Disconnected/);
});

for (const offset of [0, 10]) {
  test(`overview click loads the selected queue from cached ready offset ${offset}`, async () => {
    pagination();
    app.state.workOffset = offset;
    app.state.workPage = pageResponse(offset).data.page;
    app.state.selectedWork = `R${offset}`;
    app.state.queueTab = 'blocked';
    app.renderQueueList();
    const urls = [];
    global.fetch = async url => {
      urls.push(url);
      const response = pageResponse(0, 1, 'blocked');
      response.data.page.items = [{key: 'B1', queue: 'blocked'}];
      return {json: async () => url.startsWith('/api/work-page') ? response : {
        ok: true, data: {work: {external_key: 'B1', title: 'Blocked task', status: 'blocked'}}
      }};
    };
    await document.getElementById('queueList').children[0].click();
    await new Promise(resolve => setImmediate(resolve));
    assert.ok(urls.includes('/api/work-page?queue=blocked&offset=0&limit=10'));
    assert.equal(app.state.workFilter, 'blocked');
    assert.equal(app.state.selectedWork, 'B1');
    assert.equal(document.getElementById('detailId').textContent, 'B1');
    assert.match(document.getElementById('workRows').textContent, /B1/);
  });
}

test('second-page detail keeps its context selection through refresh and submits its key', async () => {
  pagination();
  app.state.status = app.normStatus(fixture()[0], null);
  app.state.workPage = pageResponse(10).data.page;
  app.state.workDetail = {};
  app.state.workAssess = {};
  app.state.specCheck = {};
  app.fillWorkSelect();
  const select = document.getElementById('fWork');
  select.value = 'NOT-AN-OPTION';
  assert.equal(select.value, '', 'select must reject values absent from its options');
  global.fetch = async () => ({json: async () => ({ok: true, data: {
    work: {external_key: 'R10', title: 'Ready 10', status: 'ready'}
  }})});
  await app.renderWorkDetail('R10');
  const button = document.getElementById('workDetail').find(el =>
    el.tagName === 'BUTTON' && el.textContent.includes('Compile context for this item'));
  await button.click();
  assert.equal(select.value, 'R10');
  assert.match(document.getElementById('cliMirror').textContent, /--work R10/);
  app.state.workPage = pageResponse(0).data.page;
  app.fillWorkSelect();
  assert.equal(select.value, 'R10', 'refresh must preserve the chosen context target');
  assert.equal(select.children.filter(option => option.value === 'R10').length, 1);
  let payload;
  global.fetch = async (url, options) => {
    assert.equal(url, '/api/context/compile');
    payload = JSON.parse(options.body);
    return {json: async () => ({ok: false, error: {code: 'fixture', message: 'fixture'}})};
  };
  await app.doCompile();
  assert.equal(payload.work, 'R10');
});

function overviewTarget() {
  pagination();
  app.state.queueTab = 'blocked';
  app.renderQueueList();
  return document.getElementById('queueList').children[0];
}
function changedQueue(kind = 'moved') {
  const response = pageResponse(0, 17, 'blocked');
  const blocked = kind === 'later-page'
    ? [...Array.from({length: 10}, (_, i) => ({key: `B${i + 2}`})), {key: 'B1'}]
    : kind === 'empty' ? [] : [{key: 'B2'}];
  const data = response.data;
  data.project_revision = 8;
  data.blocked = blocked.slice(0, 5);
  data.blocked_count = blocked.length;
  data.omissions.blocked = Math.max(0, blocked.length - 5);
  if (kind === 'moved' || kind === 'empty') {
    data.current.push({key: 'B1'});
    data.current_total++;
  }
  data.page = {queue: 'blocked', offset: 0, limit: 10, total: blocked.length,
    has_more: blocked.length > 10, items: blocked.slice(0, 10).map(w => ({...w, queue: 'blocked'}))};
  return response;
}
function workResponse(key) {
  return {ok: true, data: {work: {external_key: key, title: `Task ${key}`, status: 'ready'}}};
}
const settle = () => new Promise(resolve => setImmediate(resolve));

for (const kind of ['moved', 'later-page', 'empty', 'page-error', 'removed']) {
  test(`overview target survives changed queue: ${kind}`, async () => {
    const target = overviewTarget();
    const requests = [];
    global.fetch = async url => {
      requests.push(url);
      let response;
      if (url.startsWith('/api/work-page')) {
        response = kind === 'page-error'
          ? {ok: false, error: {code: 'Offline', message: 'Queue unavailable'}}
          : changedQueue(kind);
      } else {
        const key = new URL(url, 'http://local').searchParams.get('key');
        response = kind === 'removed' && key === 'B1'
          ? {ok: false, error: {code: 'NotFound', message: 'Work B1 no longer exists'}}
          : workResponse(key);
      }
      return {json: async () => response};
    };
    await target.click();
    await settle();
    assert.equal(app.state.selectedWork, 'B1');
    assert.equal(document.getElementById('detailId').textContent, 'B1');
    assert.ok(requests.includes('/api/work?key=B1'));
    assert.ok(!requests.includes('/api/work?key=B2'));
    const box = document.getElementById('workDetail');
    if (kind === 'removed') {
      assert.match(box.textContent, /Work B1 no longer exists/);
      assert.match(box.textContent, /Work item not found or removed/);
      assert.equal(box.find(el => el.tagName === 'BUTTON' && el.textContent.includes('Compile context for this item')), null);
    } else {
      assert.match(box.textContent, /Task B1/);
      const compile = box.find(el => el.tagName === 'BUTTON' && el.textContent.includes('Compile context for this item'));
      await compile.click();
      assert.equal(document.getElementById('fWork').value, 'B1');
    }
  });
}

test('a late overview detail cannot override an explicit row selection', async () => {
  const target = overviewTarget();
  let finishOldDetail;
  global.fetch = async url => {
    if (url.startsWith('/api/work-page')) return {json: async () => changedQueue()};
    if (url === '/api/work?key=B1') return new Promise(resolve => {
      finishOldDetail = () => resolve({json: async () => workResponse('B1')});
    });
    return {json: async () => workResponse('B2')};
  };
  const oldClick = target.click();
  await settle();
  assert.equal(typeof finishOldDetail, 'function');
  await document.getElementById('workRows').children[0].click();
  await settle();
  finishOldDetail();
  await oldClick;
  await settle();
  assert.equal(app.state.selectedWork, 'B2');
  assert.match(document.getElementById('workDetail').textContent, /Task B2/);
  assert.equal(app.state.raw.work.data.work.external_key, 'B2');
});

for (const navigation of ['filter', 'next-page', 'page-size']) {
  test(`explicit ${navigation} navigation releases the overview target`, async () => {
    const target = overviewTarget();
    global.fetch = async url => ({json: async () => url.startsWith('/api/work-page')
      ? changedQueue('later-page') : workResponse(new URL(url, 'http://local').searchParams.get('key'))});
    await target.click();
    await settle();
    assert.equal(app.state.selectedWork, 'B1');
    global.fetch = async url => ({json: async () => {
      if (!url.startsWith('/api/work-page')) return workResponse(new URL(url, 'http://local').searchParams.get('key'));
      const params = new URL(url, 'http://local').searchParams;
      const queue = params.get('queue'), offset = Number(params.get('offset'));
      const response = changedQueue();
      response.data.page = {queue, offset, limit: Number(params.get('limit')), total: 11,
        has_more: false, items: [{key: 'B9', queue}]};
      return response;
    }});
    if (navigation === 'filter') await document.getElementById('workFilters').children[2].click();
    else if (navigation === 'next-page') await document.getElementById('workPagination').children[2].click();
    else {
      const size = document.getElementById('workPagination').children[3];
      size.value = '20';
      size.listeners.change[0]();
    }
    await settle();
    assert.equal(app.state.selectedWork, 'B9');
    assert.match(document.getElementById('workDetail').textContent, /Task B9/);
  });
}
