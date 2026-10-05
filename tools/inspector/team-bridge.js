/**
 * Team Web bridge helpers (WS-044).
 * Demo fixtures when not live; live `--team-url` proxies to awr-server `/v1/web`
 * (cookie session, authorized query/command store, durable receipts).
 */
'use strict';

const { mapObservation, createGithubObserver } = require('./team-progress');

const fs = require('fs');
const path = require('path');

/** Owned Inspector path so browsers attach the session to /api/team/* routes. */
const OWNED_COOKIE_PATH = '/api/team';
const UPSTREAM_COOKIE_PATH = '/v1/web';

function asObject(body) {
  if (body == null) return {};
  if (typeof body === 'object') return body;
  try { return JSON.parse(body || '{}'); } catch { return null; }
}

/**
 * Rewrite upstream Set-Cookie Path=/v1/web → Path=/api/team at the proxy
 * boundary (including Max-Age=0 clears). Upstream Path would not be sent to
 * Inspector /api/team/projects, logout, or revoke.
 */
function rewriteOwnedCookiePath(cookie) {
  const raw = String(cookie);
  if (/;\s*Path=\/v1\/web(?=;|$)/i.test(raw)) {
    return raw.replace(/;\s*Path=\/v1\/web(?=;|$)/gi, `; Path=${OWNED_COOKIE_PATH}`);
  }
  // Upstream omitted Path or used another value — still pin to owned routes.
  if (/;\s*Path=/i.test(raw)) {
    return raw.replace(/;\s*Path=[^;]*/i, `; Path=${OWNED_COOKIE_PATH}`);
  }
  return `${raw}; Path=${OWNED_COOKIE_PATH}`;
}

function applyProxiedCookies(res, setCookie) {
  if (!res || !setCookie || !setCookie.length) return;
  const rewritten = setCookie.map(rewriteOwnedCookiePath);
  res.setHeader('set-cookie', rewritten.length === 1 ? rewritten[0] : rewritten);
}

function createTeamBridge(opts) {
  const live = Boolean(opts.teamUrl);
  const observeGithub = createGithubObserver(opts.githubFetch);
  let publicUrl = null;
  if (live) {
    const url = new URL(opts.teamPublicUrl || opts.teamUrl);
    if (!['http:', 'https:'].includes(url.protocol) || url.username || url.password ||
        url.search || url.hash || !/^[/a-zA-Z0-9._-]*$/.test(url.pathname)) {
      throw new Error('Team public URL must be an HTTP(S) base URL without credentials, query or fragment');
    }
    publicUrl = url.href.replace(/\/$/, '');
  }

  // Fixtures are demo-only: never when --team-url is set.
  const demoMode = !live && opts.demo !== false;
  const TEAM = {
    url: opts.teamUrl || null,
    live,
    demoMode,
    port: opts.port,
    fixtureDir:
      opts.teamFixtureDir ||
      path.resolve(__dirname, '../../tests/fixtures/workstreams/team-web-loop'),
    sessions: new Map(),
    receipts: new Map(),
  };

  function readFixture(name) {
    const file = path.join(TEAM.fixtureDir, name);
    return JSON.parse(fs.readFileSync(file, 'utf8'));
  }

  function demoSessionCookie(id) {
    return `awr_web_session=${id}; HttpOnly; Path=${OWNED_COOKIE_PATH}; SameSite=Strict`;
  }

  function parseTeamCookie(req) {
    const raw = req.headers.cookie || '';
    for (const part of raw.split(';')) {
      const p = part.trim();
      if (p.startsWith('awr_web_session=')) {
        const id = p.slice('awr_web_session='.length).trim();
        if (/^[A-Za-z0-9_-]{1,200}$/.test(id)) return id;
      }
    }
    return null;
  }

  function teamAuth(req) {
    const id = parseTeamCookie(req);
    if (!id) return null;
    const session = TEAM.sessions.get(id);
    if (!session || session.revoked || session.expires_at_ms <= Date.now()) return null;
    return session;
  }

  async function proxyTeam(reqPath, req, body, methodOverride, signal) {
    if (!TEAM.url) return null;
    const method = methodOverride || req.method || 'GET';
    const headers = {
      'content-type': 'application/json',
      'x-awr-web': '1',
      origin: `http://127.0.0.1:${TEAM.port}`,
    };
    if (req.headers && req.headers.cookie) headers.cookie = req.headers.cookie;
    const payload =
      body == null || method === 'GET' || method === 'HEAD'
        ? undefined
        : typeof body === 'string'
          ? body
          : JSON.stringify(body);
    const res = await fetch(String(TEAM.url).replace(/\/$/, '') + reqPath, {
      method,
      headers,
      body: payload,
      signal,
    });
    const text = await res.text();
    let json;
    try {
      json = JSON.parse(text);
    } catch {
      json = { ok: false, error: { code: 'BadGateway', message: text.slice(0, 200) } };
    }
    const setCookie =
      typeof res.headers.getSetCookie === 'function' ? res.headers.getSetCookie() : [];
    return { status: res.status, json, setCookie };
  }

  function liveError(proxied) {
    const body = proxied && proxied.json;
    if (body && body.code) {
      return { ok: false, error: { code: body.code, message: body.message || body.code } };
    }
    if (body && body.error) {
      return { ok: false, error: body.error };
    }
    return {
      ok: false,
      error: {
        code: proxied && proxied.status === 401 ? 'Unauthenticated' : proxied && proxied.status === 403 ? 'Forbidden' : 'UpstreamError',
        message: (body && body.message) || 'upstream request failed',
      },
    };
  }

  const ACTION_OP = {
    accept_responsibility: 'handoff.accept',
    select_agent: 'claim.acquire',
    respond_blocker: 'session.checkpoint',
    handoff_receive: 'handoff.accept',
    submit_review: 'delivery.submit_and_request_review',
    rework: 'work.rework',
    accept: 'review.accept',
  };

  function mapWorksFromQuery(data) {
    const items = (data && data.items) || (data && data.works) || [];
    if (!Array.isArray(items)) return [];
    return items.map((item) => {
      if (item && item.key) return item;
      const key = (item && (item.external_key || item.work_id || item.id)) || 'unknown';
      return {
        key: String(key),
        title: (item && (item.title || item.name)) || String(key),
        owner_person: (item && item.owner_person) || null,
        agent: (item && item.agent) || null,
        outcome: (item && item.outcome) || null,
        status: (item && (item.status || item.state)) || 'unknown',
        blocker: (item && item.blocker) || null,
        next_step: (item && item.next_step) || null,
        depends_on: (item && item.depends_on) || [],
        hidden_deps: (item && item.hidden_deps) || [],
        capabilities: (item && item.capabilities) || {},
        contract_hash: item && item.contract_hash,
        detail_loaded: false,
      };
    });
  }

  async function queryAllPages(project, query, req, res) {
    const items = [];
    const seen = new Set();
    let cursor;
    for (let page = 0; page < 100; page += 1) {
      const upstream = await proxyTeam(
        `/v1/web/projects/${encodeURIComponent(project)}/query`, req,
        { protocol_version: 1, ...query, limit: 100, ...(cursor ? { cursor } : {}) },
        'POST'
      );
      if (!upstream) return { error: { code: 'BadGateway', message: 'live proxy unavailable' } };
      applyProxiedCookies(res, upstream.setCookie);
      if (upstream.status >= 400) return liveError(upstream);
      const data = upstream.json && (upstream.json.data || upstream.json);
      if (!data || !Array.isArray(data.items)) {
        return { error: { code: 'BadGateway', message: 'invalid Team query page' } };
      }
      items.push(...data.items);
      const next = data.next_cursor;
      if (next == null) return { items };
      if (typeof next !== 'string' || !next || seen.has(next)) {
        return { error: { code: 'BadGateway', message: 'invalid Team query cursor' } };
      }
      seen.add(next);
      cursor = next;
    }
    return { error: { code: 'OverviewLimitExceeded', message: 'Team overview exceeded the page limit' } };
  }

  const routes = {
    'GET /api/team/projects': async (url, _body, req, res) => {
      if (TEAM.live) {
        const session = await proxyTeam('/v1/web/session', req, null, 'GET');
        if (!session || session.status >= 400) return liveError(session);
        applyProxiedCookies(res, session.setCookie);
        const proxied = await proxyTeam('/v1/web/projects', req, null, 'GET');
        if (!proxied) {
          return { ok: false, error: { code: 'BadGateway', message: 'live proxy unavailable' } };
        }
        applyProxiedCookies(res, proxied.setCookie);
        if (proxied.status >= 400) return liveError(proxied);
        return { ...proxied.json, session: session.json };
      }
      if (!TEAM.demoMode) {
        return { ok: false, error: { code: 'DemoDisabled', message: 'fixtures require demo mode' } };
      }
      const view = url.searchParams.get('view') === 'personal' ? 'personal-view.json' : 'team-view.json';
      const data = readFixture(view);
      const session =
        teamAuth(req) || { session_id: 'demo-anonymous', expires_at_ms: Date.now() + 3600000 };
      return { ok: true, projects: data.projects, session, view: data.view };
    },

    'GET /api/team/overview': async (url, _body, req, res) => {
      if (TEAM.live) {
        const project = url.searchParams.get('project');
        if (!project) {
          return { ok: false, error: { code: 'InvalidInput', message: 'project required' } };
        }
        const session = await proxyTeam('/v1/web/session', req, null, 'GET');
        if (!session) {
          return { ok: false, error: { code: 'BadGateway', message: 'live proxy unavailable' } };
        }
        applyProxiedCookies(res, session.setCookie);
        if (session.status >= 400) return liveError(session);

        // A member with multiple grants must select a workstream for work.list.
        // Discover only authorized streams and retain that scope on every page.
        const streams = await queryAllPages(project, { op: 'workstreams.list' }, req, res);
        if (streams.error) return { ok: false, error: streams.error };
        const works = [];
        for (const stream of streams.items) {
          if (!stream || typeof stream.id !== 'string' || !stream.id) {
            return { ok: false, error: { code: 'BadGateway', message: 'invalid Team workstream' } };
          }
          const page = await queryAllPages(
            project, { op: 'work.list', workstream_id: stream.id }, req, res
          );
          if (page.error) return { ok: false, error: page.error };
          works.push(...mapWorksFromQuery(page).map((work) => ({
            ...work, workstream_id: stream.id,
          })));
        }
        const capabilities = await proxyTeam(`/v1/web/projects/${encodeURIComponent(project)}/query`, req,
          { protocol_version: 1, op: 'capabilities' }, 'POST');
        if (!capabilities || capabilities.status >= 400) return liveError(capabilities);
        const identity = capabilities.json && (capabilities.json.data || capabilities.json).identity;
        return {
          ok: true,
          identity: identity || null,
          project,
          works,
          workstreams: streams.items.map(({ id, external_key, title }) => ({ id, external_key, title })),
          members: [],
          handoffs: [],
          reviews: [],
          schema: 'awr-team-web-loop-live/v1',
          interaction_mode: 'mcp',
          mcp_url: publicUrl + '/v1/projects/' + encodeURIComponent(project) + '/mcp',
          view_modes: ['team'],
          session: {
            session_id: session.json && session.json.session_id,
            expires_at_ms: session.json && session.json.expires_at_ms,
          },
        };
      }
      if (!TEAM.demoMode) {
        return { ok: false, error: { code: 'DemoDisabled', message: 'fixtures require demo mode' } };
      }
      const view = url.searchParams.get('view') === 'personal' ? 'personal-view.json' : 'team-view.json';
      const data = readFixture(view);
      return {
        ok: true,
        project: url.searchParams.get('project') || (data.projects[0] && data.projects[0].key),
        works: data.works,
        workstreams: data.workstreams || [],
        members: data.members,
        handoffs: data.handoffs || [],
        reviews: data.reviews || [],
        schema: data.schema,
      };
    },

    'GET /api/team/work': async (url, _body, req, res) => {
      if (!TEAM.live) return { ok: false, error: { code: 'Unsupported', message: 'Live Team detail required' } };
      const project = url.searchParams.get('project');
      const work = url.searchParams.get('work');
      const stream = url.searchParams.get('workstream');
      if (!project || !work || !stream) return { ok: false, error: { code: 'InvalidInput', message: 'project, work and workstream required' } };
      // Bound the entire detail read, including legacy fallback. This signal
      // is never applied to mutating commands with possibly unknown outcomes.
      const readSignal = AbortSignal.timeout(4000);
      const query = async op => {
        try { return await proxyTeam(`/v1/web/projects/${encodeURIComponent(project)}/query`, req, {
        protocol_version: 1, op, work_id: work, workstream_id: stream,
        ...(op === 'work.snapshot' ? { max_context_bytes: 262144 } : {}),
        }, 'POST', readSignal); } catch (error) {
          if (!readSignal.aborted) throw error;
          return { status: 504, json: { code: 'ReadTimeout', message: 'Scoped work detail timed out; retry this read' } };
        }
      };
      const unsupported = reply => [400, 501].includes(reply?.status)
        && (reply.json?.code === 'Unsupported' || reply.json?.error?.code === 'Unsupported');
      let upstream = await query('work.snapshot');
      applyProxiedCookies(res, upstream?.setCookie);
      const legacy = unsupported(upstream);
      if (legacy) upstream = await query('work.prepare');
      if (!upstream || upstream.status >= 400) return liveError(upstream);
      applyProxiedCookies(res, upstream.setCookie);
      const data = upstream.json && upstream.json.data;
      if (!data || data.work_id !== work || !data.visible_contract || upstream.json.workstream_id !== stream) {
        return { ok: false, error: { code: 'BadGateway', message: 'Invalid scoped Team work detail' } };
      }
      const expected = url.searchParams.get('contract');
      if (expected && expected !== data.contract_hash) {
        return { ok: false, error: { code: 'SourceChanged', message: 'Work contract changed; refresh the overview' } };
      }
      const observed = legacy ? await query('work.observe') : upstream;
      if (!observed) return liveError(observed);
      if (legacy) applyProxiedCookies(res, observed.setCookie);
      let progress = { observation_available: false, observation_error: 'unsupported' };
      let snapshot = { consistency: 'unconfirmed_legacy' };
      if (observed.status >= 400) {
        if (!legacy || !unsupported(observed)) return liveError(observed);
      } else {
        const observation = legacy ? observed.json?.data : data.observation;
        if (!observation || observation.work_id !== work || observed.json.workstream_id !== stream
          || observation.contract_hash !== data.contract_hash || !Number.isFinite(observation.observed_at_unix_ms)) {
          return { ok: false, error: { code: legacy ? 'SourceChanged' : 'BadGateway', message: 'Invalid scoped observation; refresh the project' } };
        }
        if (!legacy) {
          const meta = data.snapshot, envelope = upstream.json;
          const fields = ['state', 'work_version', 'last_fence', 'recovery_blocked', 'selected_completion_id'];
          const validRuntime = value => value && !Array.isArray(value)
            && typeof value.state === 'string' && value.state
            && typeof value.work_version === 'string' && /^\d+$/.test(value.work_version)
            && typeof value.last_fence === 'string' && /^\d+$/.test(value.last_fence)
            && typeof value.recovery_blocked === 'boolean'
            && (value.selected_completion_id === null || typeof value.selected_completion_id === 'string');
          const runtimeMatches = data.runtime === null && observation.runtime === null
            || validRuntime(data.runtime) && validRuntime(observation.runtime)
              && fields.every(k => data.runtime[k] === observation.runtime[k]);
          if (meta?.version !== 1 || meta.consistency !== 'repeatable_read'
            || typeof meta.project_revision !== 'string' || !/^\d+$/.test(meta.project_revision)
            || meta.project_revision !== envelope.project_revision
            || typeof meta.source_snapshot_id !== 'string' || !meta.source_snapshot_id
            || meta.source_snapshot_id !== envelope.source_snapshot_id
            || typeof meta.coordinator_epoch !== 'string' || !meta.coordinator_epoch
            || meta.coordinator_epoch !== envelope.coordinator_epoch
            || meta.queried_at_unix_ms !== observation.observed_at_unix_ms
            || typeof data.context_complete !== 'boolean' || !runtimeMatches) {
            return { ok: false, error: { code: 'BadGateway', message: 'Invalid atomic Team snapshot' } };
          }
          snapshot = meta;
        }
        progress = mapObservation(observation);
        progress.github = observeGithub.background(progress.pr_reference);
      }
      // prepare may omit its optional hint to respect the caller's byte budget.
      // Its required completeness fact still takes priority over observe advice.
      if (data.context_complete === false) {
        progress.guidance = data.guidance?.code === 'restore_context' ? data.guidance
          : { code: 'restore_context', action: { op: 'work.prepare', note: data.next_step || null } };
        progress.next_step = progress.guidance.action?.note || null;
      }
      return { ok: true, work: {
        key: work, workstream_id: stream, contract_hash: data.contract_hash,
        detail_loaded: true, status: data.runtime ? data.runtime.state : null,
        runtime_available: Boolean(data.runtime),
        recovery_blocked: Boolean(data.runtime && data.runtime.recovery_blocked),
        acceptance: data.visible_contract.acceptance || [],
        goals: data.visible_contract.goals || [],
        depends_on: (data.visible_contract.required_dependencies || []).map((key) => ({ key, visible: true })),
        dependency_export_unavailable: data.dependency_export_unavailable === true,
        context_complete: data.context_complete === true,
        completeness_reasons: data.completeness_reasons || [],
        next_step: data.next_step || null,
        execution_admission: data.execution_admission || 'not_evaluated',
        ...progress,
        snapshot,
      } };
    },

    'POST /api/team/access': async (_url, body, req, res) => {
      if (!TEAM.live) return { ok: false, error: { code: 'Unsupported', message: 'Live Team access required' } };
      const input = asObject(body);
      if (!input || typeof input.project !== 'string' || !input.project ||
          !['inspect', 'preview', 'apply', 'outcome'].includes(input.operation) ||
          !input.payload || typeof input.payload !== 'object' || Array.isArray(input.payload)) {
        return { ok: false, error: { code: 'InvalidInput', message: 'Invalid access operation' } };
      }
      const upstream = await proxyTeam(`/v1/web/projects/${encodeURIComponent(input.project)}/access/${input.operation}`,
        req, input.payload, 'POST');
      if (!upstream || upstream.status >= 400) return liveError(upstream);
      applyProxiedCookies(res, upstream.setCookie);
      return { ok: true, data: upstream.json };
    },

    'GET /api/team/activity': async (url, _body, req, res) => {
      if (!TEAM.live) return { ok: false, error: { code: 'Unsupported', message: 'Live Team audit required' } };
      const project = url.searchParams.get('project');
      const kind = url.searchParams.get('kind');
      if (!project || !['requests', 'development'].includes(kind)) {
        return { ok: false, error: { code: 'InvalidInput', message: 'Project and activity kind required' } };
      }
      const query = { protocol_version: 1, op: 'audit.' + kind, limit: 50 };
      for (const key of ['cursor', 'member_actor_id', 'work_id']) {
        const value = url.searchParams.get(key);
        if (value) query[key] = value;
      }
      const upstream = await proxyTeam(`/v1/web/projects/${encodeURIComponent(project)}/query`, req, query, 'POST');
      if (!upstream || upstream.status >= 400) return liveError(upstream);
      applyProxiedCookies(res, upstream.setCookie);
      return { ok: true, data: upstream.json.data || upstream.json };
    },

    'POST /api/team/login': async (_url, body, req, res) => {
      if (TEAM.live) {
        const proxied = await proxyTeam('/v1/web/login', req, body, 'POST');
        if (!proxied) {
          return { ok: false, error: { code: 'BadGateway', message: 'live proxy unavailable' } };
        }
        applyProxiedCookies(res, proxied.setCookie);
        if (proxied.status >= 400) return liveError(proxied);
        return proxied.json;
      }
      if (!TEAM.demoMode) {
        return { ok: false, error: { code: 'DemoDisabled', message: 'fixtures require demo mode' } };
      }
      const parsed = asObject(body);
      if (!parsed) {
        return { ok: false, error: { code: 'InvalidInput', message: 'invalid login' } };
      }
      if (!parsed.bearer || typeof parsed.bearer !== 'string' || parsed.bearer.length < 8) {
        return { ok: false, error: { code: 'Forbidden', message: 'access denied' } };
      }
      const id = 'ws_demo_' + Date.now().toString(36);
      const session = {
        session_id: id,
        bearer_present: true,
        expires_at_ms: Date.now() + 8 * 3600 * 1000,
        revoked: false,
      };
      TEAM.sessions.set(id, session);
      res.setHeader('set-cookie', demoSessionCookie(id));
      return {
        ok: true,
        protocol: 'awr-team-web-entry',
        protocol_version: 1,
        session_id: id,
        expires_at_ms: session.expires_at_ms,
        auth: { kind: 'http_only_cookie', bearer_in_page: false },
      };
    },

    'POST /api/team/logout': async (_url, _body, req, res) => {
      if (TEAM.live) {
        const proxied = await proxyTeam('/v1/web/logout', req, '{}', 'POST');
        if (!proxied) {
          return { ok: false, error: { code: 'BadGateway', message: 'live proxy unavailable' } };
        }
        applyProxiedCookies(res, proxied.setCookie);
        if (proxied.status >= 400) return liveError(proxied);
        return proxied.json;
      }
      const id = parseTeamCookie(req);
      if (id && TEAM.sessions.has(id)) TEAM.sessions.get(id).revoked = true;
      res.setHeader(
        'set-cookie',
        `awr_web_session=; HttpOnly; Path=${OWNED_COOKIE_PATH}; SameSite=Strict; Max-Age=0`
      );
      return { ok: true, logged_out: true };
    },

    'POST /api/team/session/revoke': async (_url, body, req, res) => {
      if (TEAM.live) {
        const proxied = await proxyTeam('/v1/web/session/revoke', req, body, 'POST');
        if (!proxied) {
          return { ok: false, error: { code: 'BadGateway', message: 'live proxy unavailable' } };
        }
        applyProxiedCookies(res, proxied.setCookie);
        if (proxied.status >= 400) return liveError(proxied);
        return proxied.json;
      }
      const id = parseTeamCookie(req);
      if (id && TEAM.sessions.has(id)) TEAM.sessions.get(id).revoked = true;
      for (const s of TEAM.sessions.values()) s.revoked = true;
      res.setHeader(
        'set-cookie',
        `awr_web_session=; HttpOnly; Path=${OWNED_COOKIE_PATH}; SameSite=Strict; Max-Age=0`
      );
      return { ok: true, revoked: [{ scope: 'all_mine' }] };
    },

    'POST /api/team/action': async (_url, body, req, res) => {
      const parsed = asObject(body);
      if (!parsed) {
        return { ok: false, error: { code: 'InvalidInput', message: 'invalid action' } };
      }

      if (TEAM.live) {
        if (!parsed.project || typeof parsed.project !== 'string') {
          return { ok: false, error: { code: 'InvalidInput', message: 'project required' } };
        }
        // Live writes must hit the authorized command store — never invent receipts.
        const session = await proxyTeam('/v1/web/session', req, null, 'GET');
        if (!session) {
          return { ok: false, error: { code: 'BadGateway', message: 'live proxy unavailable' } };
        }
        applyProxiedCookies(res, session.setCookie);
        if (session.status >= 400) return liveError(session);

        let commandPayload = parsed.command;
        if (!commandPayload || typeof commandPayload !== 'object') {
          // Allow callers to supply a full command under top-level fields.
          if (parsed.protocol_version && parsed.op && parsed.request_id) {
            commandPayload = { ...parsed };
            delete commandPayload.project;
            delete commandPayload.action;
            delete commandPayload.command;
            delete commandPayload.expired;
          } else {
            return {
              ok: false,
              error: {
                code: 'InvalidInput',
                message:
                  'live action requires a command payload for the authorized command store',
              },
            };
          }
        }
        if (parsed.expired === true) {
          return { ok: false, error: { code: 'ExpiredOperation', message: 'operation expired' } };
        }

        const proxied = await proxyTeam(
          `/v1/web/projects/${encodeURIComponent(parsed.project)}/command`,
          req,
          commandPayload,
          'POST'
        );
        if (!proxied) {
          return { ok: false, error: { code: 'BadGateway', message: 'live proxy unavailable' } };
        }
        applyProxiedCookies(res, proxied.setCookie);
        if (proxied.status >= 400) return liveError(proxied);

        const upstream = proxied.json || {};
        // Uncertain-outcome / exact replay: forward store receipt unchanged.
        return {
          ok: true,
          replayed: Boolean(upstream.replayed),
          receipt: upstream.receipt || null,
          server_op: (upstream.receipt && upstream.receipt.op) || commandPayload.op || null,
          execution_authorized: upstream.execution_authorized,
          upstream,
        };
      }

      if (!TEAM.demoMode) {
        return { ok: false, error: { code: 'DemoDisabled', message: 'fixtures require demo mode' } };
      }

      const allowed = new Set(Object.keys(ACTION_OP));
      if (!allowed.has(parsed.action) || !parsed.request_id || !parsed.work_key) {
        return { ok: false, error: { code: 'InvalidInput', message: 'invalid action fields' } };
      }
      if (parsed.expired === true) {
        return { ok: false, error: { code: 'ExpiredOperation', message: 'operation expired' } };
      }
      const receiptKey = parsed.request_id;
      if (TEAM.receipts.has(receiptKey)) {
        return { ok: true, replayed: true, receipt: TEAM.receipts.get(receiptKey) };
      }
      const receipt = {
        id: 'rcpt_' + receiptKey,
        request_id: receiptKey,
        op: ACTION_OP[parsed.action],
        work_key: parsed.work_key,
        project: parsed.project,
        agent_id: parsed.agent_id || null,
        at_ms: Date.now(),
      };
      TEAM.receipts.set(receiptKey, receipt);
      return { ok: true, replayed: false, receipt, server_op: receipt.op };
    },
  };

  return {
    TEAM,
    routes,
    readFixture,
    teamAuth,
    rewriteOwnedCookiePath,
    OWNED_COOKIE_PATH,
    UPSTREAM_COOKIE_PATH,
  };
}

module.exports = { createTeamBridge, rewriteOwnedCookiePath };
