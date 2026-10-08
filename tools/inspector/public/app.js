/* ══════════════════════════════════════════════════════════
   AWR Console: frontend logic
   ══════════════════════════════════════════════════════════ */

(function () {
  'use strict';

  const i18n = typeof module !== 'undefined' && module.exports
    ? require('./i18n.js') : window.AWR_I18N;

  // ───────────────────────────────────────────────────────
  // Field mappings
  //
  // Paths for status, work show, and context compile were checked against:
  //   crates/awr-runtime/src/status_action.rs   （status --view action）
  //   crates/awr-runtime/src/status_summary.rs (queue entry brief shape)
  //   crates/awr-cli/src/query.rs               （work show）
  //   crates/awr-context/src/{compile,budget}.rs（context compile）
  // The intake inspect shape has not been fully checked; Sources mappings remain provisional.
  //
  // Each field has candidate paths; use the first available value. If a cell shows a dash,
  // inspect that view's raw JSON and add the actual field path to the corresponding array.
  // Keep all field mappings here instead of guessing in rendering code.
  // ───────────────────────────────────────────────────────

  const FIELD_MAP = {
    status: {
      revision:     ['project_revision', 'revision'],
      currentTotal: ['current_total'],
      readyCount:   ['ready_count', 'counts.ready'],
      waitingCount: ['waiting_count'],
      blockedCount: ['blocked_count', 'counts.blocked'],
      selected:     ['total'],
      projectTotal: ['project_work_total'],
      gaps:         ['organization.gaps'],
      pendingOps:   ['pending_operations.items'],
      pendingTotal: ['pending_operations.total'],
      gapTotal:     ['organization.gap_total', 'organization.project_gap_total'],
      orgState:     ['organization.state'],
      freshness:    ['freshness_basis'],
      guidance:     ['guidance.next_action', 'next_action'],
    },
    // Support both status shapes:
    //   - Source tree: current/ready/waiting/blocked arrays plus omissions.
    //   - Release 0.4.0: current only; obtain ready and blocked through awr ready.
    queueItems:     { current: ['current'], ready: ['ready'], waiting: ['waiting'], blocked: ['blocked'] },
    queueOmitted:   { current: ['omissions.current'], ready: ['omissions.ready'], waiting: ['omissions.waiting'], blocked: ['omissions.blocked'] },
    // The awr ready response.
    readyCmd: {
      items:        ['ready'],
      total:        ['ready_total'],
      blockedItems: ['blocked_sample'],
      // blocked_total means unselectable, including claimed work, unlike status.blocked_count.
      // Use it only for list truncation, not the status strip.
      blockedTotal: ['blocked_total'],
    },
    workItem: {
      key:        ['key', 'external_key', 'work', 'id'],
      title:      ['title', 'summary', 'name'],
      status:     ['status', 'state'],
      rawStatus:  ['raw_status'],
      owner:      ['owner'],
      queue:      ['queue', 'bucket'],
      revision:   ['revision', 'source_revision'],
      nextAction: ['next_action', 'next'],
      blocker:    ['blocker', 'block_reason'],
      waitTotal:  ['wait_total'],
      codes:      ['codes', 'structural_codes'],
      // Fields available only from work show.
      goal:       ['milestone', 'goal', 'goal_summary'],
      acceptance: ['acceptance', 'acceptance_criteria', 'criteria'],
      dependsOn:  ['required_dependencies', 'depends_on', 'dependencies'],
      missingDeps:['missing_dependencies'],
      claims:     ['active_claims'],
      diagnostics:['diagnostics'],
      evidence:   ['evidence'],
      decisions:  ['decisions'],
      cycles:     ['dependency_cycles'],
    },
    context: {
      rendered:   ['work_context.rendered_context', 'rendered_context', 'diagnostic_text'],
      chunks:     ['work_context.selected_chunks', 'selected_chunks'],
      total:      ['work_context.token_estimate', 'token_estimate'],
      required:   ['work_context.required_tokens', 'required_tokens'],
      budget:     ['work_context.token_budget', 'token_budget'],
      hash:       ['work_context.context_hash', 'context_hash'],
      tokenizer:  ['work_context.tokenizer', 'tokenizer'],
      complete:   ['completeness.complete', 'ok'],
      statusText: ['completeness.status'],
      revision:   ['completeness.project_revision', 'project_revision'],
      omissions:  ['work_context.omitted_chunks', 'omitted_refs', 'omitted_chunks'],
      // Completeness has separate dimensions; missing facts affect safe execution.
      dimensions: ['completeness'],
      evidenceGaps: ['completeness.evidence_gaps'],
      unresolvedDeps: ['completeness.unresolved_required_dependencies'],
      issues:     ['completeness.issues'],
    },
    // awr intake inspect returns an organization report, not a file table.
    // The source list is organization.sources[]: {domain, freshness, locator, revision, role}.
    sources: {
      files:      ['organization.sources', 'files', 'sources', 'matched'],
      path:       ['locator', 'path', 'file'],
      kind:       ['domain', 'kind', 'type'],
      state:      ['freshness', 'state', 'status'],
      role:       ['role'],
      revision:   ['revision', 'source_revision'],
      rejection:  ['rejection', 'violation', 'error'],
      issues:     ['source_issues'],
      gapTotal:   ['organization.gap_total'],
    },
    // awr session list --json returns Session structs from awr-core.
    session: {
      id:         ['id'],
      agent:      ['agent_id', 'agent'],
      status:     ['status'],
      work:       ['work_item_id'],
      provider:   ['provider'],
      model:      ['model'],
      checkpoint: ['last_checkpoint_id'],
      startedAt:  ['started_at'],
    },
    // event history uses event_brief: type, not event_type.
    event: {
      id:         ['id'],
      type:       ['type', 'event_type'],
      summary:    ['summary'],
      importance: ['importance'],
      session:    ['session_id'],
      work:       ['work_item_id'],
      createdAt:  ['created_at'],
    },
  };

  /** Read a dotted path, including array indices such as a.b.0.c. */
  function at(obj, path) {
    if (obj == null) return undefined;
    return path.split('.').reduce((o, k) => (o == null ? undefined : o[k]), obj);
  }

  /** Return the first non-null value from the candidate paths. */
  function pick(obj, candidates, fallback) {
    for (const p of candidates || []) {
      const v = at(obj, p);
      if (v !== undefined && v !== null && v !== '') return v;
    }
    return fallback;
  }

  // Utilities

  const $ = (id) => document.getElementById(id);
  const el = (tag, cls, text) => {
    const n = document.createElement(tag);
    if (cls) n.className = cls;
    if (text != null) n.textContent = text;
    return n;
  };
  const group = (n) => (n == null || isNaN(n) ? '—' : Math.round(n).toLocaleString(i18n.locale));

  /** Render a relative timestamp; preserve the original value if it cannot be parsed. */
  function since(value) {
    if (!value) return '—';
    const t = typeof value === 'number' ? value : Date.parse(value);
    if (isNaN(t)) return String(value);
    const min = Math.max(0, Math.round((Date.now() - t) / 60000));
    if (min < 1) return i18n.t('ui.just_now');
    if (min < 60) return i18n.t('ui.p0_min_ago', { p0: min });
    const h = Math.round(min / 60);
    if (h < 24) return i18n.t('ui.p0_hr_ago', { p0: h });
    const d = Math.round(h / 24);
    return i18n.t('ui.p0_days_ago', { p0: d });
  }

  /** Render future timestamps such as claim expiration; mark past values as expired. */
  function until(value) {
    if (!value) return '';
    const t = typeof value === 'number' ? value : Date.parse(value);
    if (isNaN(t)) return String(value);
    const min = Math.round((t - Date.now()) / 60000);
    if (min <= 0) return i18n.t('ui.expired');
    if (min < 60) return i18n.t('ui.expires_in_p0_min', { p0: min });
    const h = Math.round(min / 60);
    if (h < 24) return i18n.t('ui.expires_in_p0_hr', { p0: h });
    return i18n.t('ui.expires_in_p0_days', { p0: Math.round(h / 24) });
  }

  function setText(id, text) {
    const n = $(id);
    if (n) n.textContent = text;
  }

  function clear(node) {
    while (node && node.firstChild) node.removeChild(node.firstChild);
  }

  // State

  const state = {
    mode: 'demo',          // 'live' | 'demo'
    project: '…',
    reason: null,
    workPagination: false,
    workPage: null,
    workOffset: 0,
    workPageSize: 10,
    workPageError: null,
    workPageLoading: false,
    status: null,          // Normalized status.
    works: [],             // Normalized work-item list.
    workDetail: {},        // Key to details.
    sources: null,
    sessions: { items: [], loaded: false, error: null, command: null, mayHaveMore: false },
    events: { items: [], loaded: false, error: null, command: null, mayHaveMore: false },
    unsupported: new Set(),
    compile: null,
    raw: {},               // Most recent raw JSON for each view.
    queueTab: 'blocked',   // Selected queue in the overview.
    workFilter: 'all',
    selectedWork: null,
    overviewWork: null,    // Explicit Overview selection; independent of the current page.
    view: 'overview',
  };

  const QUEUES = [
    { key: 'current', label: i18n.t('ui.in_progress'), dot: 'info', why: i18n.t('ui.someone_is_working_on_it') },
    { key: 'ready',   label: i18n.t('ui.ready'), dot: 'ok',   why: i18n.t('ui.dependencies_are_satisfied') },
    { key: 'waiting', label: i18n.t('ui.waiting'), dot: 'warn', why: i18n.t('ui.waiting_for_a_reply_or_prerequisite') },
    { key: 'blocked', label: i18n.t('ui.blocked'), dot: 'crit', why: i18n.t('ui.cannot_proceed') },
  ];
  const queueMeta = (k) => QUEUES.find((q) => q.key === k) || { label: k || '—', dot: '', why: '' };

  /** Keep the budget limit aligned with the bridge and awr-context/src/budget.rs. */
  const BUDGET_MAX = 100000;

  // Generation guards

  /**
   * Guard concurrent detail requests by generation.
   *
   * If A is selected before B but responds after B, A must not replace B's panel
   * or make B's compile button target the wrong item.
   *
   * Give each request an increasing token and accept only the latest still-selected item.
   * Call invalidate() on refresh to reject all older in-flight requests.
   */
  function createGenerationGuard() {
    let generation = 0;
    let currentKey = null;
    return {
      begin(key) {
        generation += 1;
        currentKey = key;
        return { generation, key };
      },
      isCurrent(token) {
        return Boolean(token) && token.generation === generation && token.key === currentKey;
      },
      invalidate() {
        generation += 1;
        currentKey = null;
      },
    };
  }

  const detailGuard = createGenerationGuard();
  let sourceGeneration = 0;

  // ───────────────────────── API ─────────────────────────

  // The bridge requires this header on mutations. Third-party pages trigger a CORS
  // preflight that the bridge rejects, so it also provides CSRF protection.
  const GUARD_HEADER = 'X-AWR-Inspector';

  async function callApi(path, options) {
    const opts = Object.assign({}, options);
    opts.headers = Object.assign(
      { 'content-type': 'application/json', [GUARD_HEADER]: '1' },
      opts.headers
    );
    try {
      const res = await fetch(path, opts);
      return await res.json();
    } catch (err) {
      return { ok: false, error: { code: 'BridgeUnreachable', message: String(err.message) } };
    }
  }

  // A static file of the page itself (the Project map puts the same module files into the HTML it downloads).
  async function fetchAsset(path) {
    const res = await fetch(path);
    if (!res.ok) throw new Error(path + ': ' + res.status);
    return res.text();
  }

  // Normalization

  function normWorkBrief(raw) {
    const M = FIELD_MAP.workItem;
    const waitTotal = pick(raw, M.waitTotal, 0);
    return {
      key: pick(raw, M.key, '—'),
      title: pick(raw, M.title, i18n.t('ui.no_title_in_the_source')),
      status: pick(raw, M.status, null),
      rawStatus: pick(raw, M.rawStatus, null),
      owner: pick(raw, M.owner, null),
      queue: pick(raw, M.queue, null),
      revision: pick(raw, M.revision, null),
      nextAction: pick(raw, M.nextAction, null),
      blocker: pick(raw, M.blocker, null),
      waitTotal: waitTotal,
      // Diagnostic codes explain blockers, for example dependency_not_completed.
      codes: pick(raw, M.codes, []) || [],
      raw,
    };
  }

  function normWorkDetail(raw) {
    const M = FIELD_MAP.workItem;
    // work show nests the item under work; acceptance and dependencies are siblings.
    const item = raw && raw.work ? raw.work : raw;
    const base = normWorkBrief(item);

    // Acceptance is Vec<String>: criterion text without completion flags.
    // Evidence is checked per criterion at completion; this view cannot infer success.
    const acceptance = (pick(raw, M.acceptance, []) || []).map((a) =>
      typeof a === 'string'
        ? { criterion: a, evidence: [] }
        : { criterion: a.criterion || a.text || String(a), evidence: a.evidence || [] }
    );

    const deps = (pick(raw, M.dependsOn, []) || []).map((d) =>
      typeof d === 'string' ? { key: d } : { key: d.external_key || d.key, status: d.status }
    );

    return Object.assign(base, {
      // Milestone and goal belong to the item, not the response envelope.
      goal: pick(item, M.goal, null) || pick(raw, M.goal, null),
      acceptance,
      dependsOn: deps,
      missingDeps: pick(raw, M.missingDeps, []) || [],
      cycles: pick(raw, M.cycles, []) || [],
      // Active claims identify ownership; an agent needs a claim before starting work.
      claims: (pick(item, M.claims, []) || []).map((c) => ({
        id: c.id,
        session: c.session_id || c.session,
        agent: c.agent_id || c.agent,
        expiresAt: c.expires_at || null,
      })),
      diagnostics: pick(item, M.diagnostics, []) || [],
      evidence: pick(raw, M.evidence, []) || [],
      decisions: pick(raw, M.decisions, []) || [],
    });
  }

  /**
   * @param raw       The awr status response.
   * @param readyRaw  The awr ready response supplements 0.4.0 status, which lacks
   *                  ready/blocked arrays. Matching revisions can fill action-view summaries.
   */
  function normStatus(raw, readyRaw) {
    const M = FIELD_MAP.status;
    const R = FIELD_MAP.readyCmd;
    const queues = {};
    let all = [];

    // Prefer queue arrays from status, falling back to awr ready.
    const fallback = {
      ready: pick(readyRaw, R.items, null),
      blocked: pick(readyRaw, R.blockedItems, null),
    };

    for (const q of QUEUES) {
      let items = pick(raw, FIELD_MAP.queueItems[q.key], null);
      let omitted = pick(raw, FIELD_MAP.queueOmitted[q.key], 0) || 0;
      let source = 'status';

      if (fallback[q.key] != null && (items == null || (
        q.key === 'ready' && omitted > 0 &&
        raw.project_revision === readyRaw.project_revision &&
        fallback.ready.length > items.length
      ))) {
        items = fallback[q.key];
        source = 'ready';
        if (q.key === 'ready') {
          const t = pick(readyRaw, R.total, items.length);
          omitted = Math.max(0, t - items.length);
        }
      }
      // Release 0.4.0 has no waiting queue; do not represent missing data as zero.
      const available = items != null;
      items = items || [];

      const list = items.map(normWorkBrief).map((w) => Object.assign(w, { queue: q.key }));
      queues[q.key] = { total: list.length + omitted, omitted, items: list, available, source };
      all = all.concat(list);
    }

    // Use AWR counts, not inferred list lengths. Represent missing counts as null.
    const counted = {
      current: pick(raw, M.currentTotal, queues.current.available ? queues.current.total : null),
      ready: pick(raw, M.readyCount, queues.ready.available ? queues.ready.total : null),
      waiting: pick(raw, M.waitingCount, queues.waiting.available ? queues.waiting.total : null),
      blocked: pick(raw, M.blockedCount, queues.blocked.available ? queues.blocked.total : null),
    };
    for (const k of Object.keys(counted)) {
      if (counted[k] != null) queues[k].total = counted[k];
      else queues[k].available = false;
    }

    // Deduplicate keys across queues; keep the first occurrence.
    const seen = new Set();
    const works = all.filter((w) => (seen.has(w.key) ? false : (seen.add(w.key), true)));

    return {
      revision: pick(raw, M.revision, null),
      currentTotal: counted.current,
      readyCount: counted.ready,
      waitingCount: counted.waiting,
      blockedCount: counted.blocked,
      selected: pick(raw, M.selected, null),
      projectTotal: pick(raw, M.projectTotal, null),
      queues,
      works,
      gaps: pick(raw, M.gaps, []) || [],
      gapTotal: pick(raw, M.gapTotal, null),
      // Interrupted operations with unknown outcomes must be inspected before retrying.
      pendingOps: pick(raw, M.pendingOps, []) || [],
      pendingTotal: pick(raw, M.pendingTotal, null),
      orgState: pick(raw, M.orgState, null),
      freshness: pick(raw, M.freshness, null),
      guidance: pick(raw, M.guidance, null),
    };
  }

  function normSources(raw) {
    const M = FIELD_MAP.sources;
    const files = (pick(raw, M.files, []) || []).map((f) => {
      const locator = pick(f, M.path, '—');
      return {
        // Display file:// source locators as readable project-relative paths.
        path: shortenLocator(locator),
        locator,
        kind: pick(f, M.kind, '—'),
        role: pick(f, M.role, null),
        revision: pick(f, M.revision, null),
        // AWR source state is named freshness: fresh, stale, etc.
        state: String(pick(f, M.state, 'fresh')).toLowerCase(),
        rejection: pick(f, M.rejection, null),
      };
    });
    // Summarize by domain instead of file extension.
    const summary = {};
    for (const f of files) summary[f.kind] = (summary[f.kind] || 0) + 1;

    return {
      files,
      summary: files.length ? summary : null,
      issues: pick(raw, M.issues, []) || [],
      gapTotal: pick(raw, M.gapTotal, null),
    };
  }

  /** Convert file:///a/b/demo/RULES.md to demo/RULES.md; preserve unrecognized values. */
  function shortenLocator(locator) {
    const s = String(locator || '');
    if (!s.startsWith('file://')) return s;
    const parts = s.replace('file://', '').split('/').filter(Boolean);
    return parts.slice(-2).join('/') || s;
  }

  function normContext(raw) {
    const M = FIELD_MAP.context;

    // AWR supplies selected_chunks with section/required, not per-section token counts.
    // Group the chunk counts by section and show how many are required;
    // do not invent token counts that the API does not provide.
    const chunks = pick(raw, M.chunks, []) || [];
    const bySection = new Map();
    for (const c of chunks) {
      const name = String(c.section != null ? c.section : i18n.t('ui.unsectioned'));
      const row = bySection.get(name) || { name, count: 0, required: 0 };
      row.count += 1;
      if (c.required) row.required += 1;
      bySection.set(name, row);
    }
    const sections = [...bySection.values()].sort((a, b) => b.count - a.count);

    const omissions = (pick(raw, M.omissions, []) || []).map((o) =>
      typeof o === 'string'
        ? { detail: o, key: o, section: null, reason: null }
        : {
            key: o.key || null,
            section: o.section != null ? String(o.section) : null,
            reason: o.reason || null,
            detail: [o.key, o.section].filter((x) => x != null).join(' · ') || JSON.stringify(o),
          }
    );

    // Show each completeness dimension so missing requirements are visible.
    const DIMS = [
      ['rules_complete', i18n.t('ui.rules')],
      ['goal_context_complete', i18n.t('ui.goal_context')],
      ['work_state_complete', i18n.t('ui.work_state')],
      ['acceptance_complete', i18n.t('ui.acceptance_criteria')],
      ['dependencies_complete', i18n.t('ui.dependencies')],
      ['source_fresh', i18n.t('ui.source_freshness')],
    ];
    const c = pick(raw, M.dimensions, {}) || {};
    const dimensions = DIMS
      .filter(([k]) => c[k] !== undefined)
      .map(([k, label]) => ({ key: k, label, ok: Boolean(c[k]) }));

    return {
      rendered: pick(raw, M.rendered, ''),
      work: pick(raw, ['work_context.identity.work_item_key'], null),
      sections,
      chunkTotal: chunks.length,
      dimensions,
      statusText: pick(raw, M.statusText, null),
      evidenceGaps: pick(raw, M.evidenceGaps, []) || [],
      unresolvedDeps: pick(raw, M.unresolvedDeps, []) || [],
      issues: pick(raw, M.issues, []) || [],
      total: pick(raw, M.total, null),
      requiredTokens: pick(raw, M.required, null),
      budget: pick(raw, M.budget, null),
      hash: pick(raw, M.hash, null),
      tokenizer: pick(raw, M.tokenizer, null),
      revision: pick(raw, M.revision, null),
      complete: pick(raw, M.complete, null),
      omissions,
    };
  }

  // Shared rendering blocks

  function stateBlock(kind, title, msg, command) {
    const box = el('div', 'state' + (kind === 'err' ? ' err' : ''));
    box.appendChild(el('div', 'title', title));
    if (msg) box.appendChild(el('div', 'msg', msg));
    if (command) {
      const cmd = el('div', 'cmd');
      cmd.appendChild(el('span', 'prompt', '$'));
      cmd.appendChild(el('code', null, command));
      const btn = el('button', 'copy', i18n.t('ui.copy'));
      btn.addEventListener('click', () => copyText(command, btn));
      cmd.appendChild(btn);
      box.appendChild(cmd);
    }
    return box;
  }

  function errorBlock(error, command) {
    const code = (error && error.code) || 'Error';
    const msg = (error && error.message) || i18n.t('ui.no_further_information');
    const advice = {
      SourceStale: i18n.t('ui.source_files_have_changed_and_the_projection'),
      RevisionConflict: i18n.t('ui.project_state_changed_during_this_operation_refresh'),
      DemoMode: i18n.t('ui.demo_mode_is_active_the_data_below'),
      BridgeUnreachable: i18n.t('ui.cannot_reach_the_local_bridge_check_that'),
      NotJson: i18n.t('ui.awr_returned_non_json_output_expand_raw'),
      BudgetExceeded: i18n.t('ui.required_content_exceeds_the_budget_awr_will'),
      OutcomeUnknown: i18n.t('ui.the_command_was_not_terminated_and_may'),
      BridgeTimeout: i18n.t('ui.the_read_only_command_timed_out_and'),
      ReindexNotAllowed: i18n.t('ui.reindexing_is_disabled_by_default_restart_the'),
      OutputTooLarge: i18n.t('ui.the_output_exceeds_the_bridge_limit_run'),
      BridgeBusy: i18n.t('ui.too_many_commands_are_running_wait_for'),
      ForbiddenHost: i18n.t('ui.the_request_host_is_not_an_accepted'),
      ForbiddenOrigin: i18n.t('ui.the_request_came_from_another_origin_and'),
      MissingGuardHeader: i18n.t('ui.the_mutation_request_is_missing_its_guard'),
    }[code];

    const box = el('div', 'state err');
    const t = el('div', 'title');
    t.appendChild(el('span', 'errcode', code));
    box.appendChild(t);
    box.appendChild(el('div', 'msg', msg));
    if (advice) box.appendChild(el('div', 'msg', advice));

    // Offer a retry using the required token count returned by AWR.
    const required = error && error.details && Number(error.details.required);
    if (code === 'BudgetExceeded' && Number.isFinite(required)) {
      if (required > BUDGET_MAX) {
        // Required content exceeds the hard limit; no larger budget can succeed.
        box.appendChild(el('div', 'msg',
          i18n.t('ui.required_exceeds_limit', { required: group(required), limit: group(BUDGET_MAX) })));
      } else {
        // Leave 10% headroom without exceeding the limit enforced by the bridge.
        const target = Math.min(BUDGET_MAX, Math.ceil((required * 1.1) / 500) * 500);
        const act = el('div', 'actions');
        const bump = el('button', 'btn', i18n.t('ui.retry_budget', { budget: group(target) }));
        bump.addEventListener('click', () => {
          $('fBudget').value = String(target);
          updateCliMirror();
          doCompile();
        });
        act.appendChild(bump);
        box.appendChild(act);
      }
    }
    if (command) {
      const cmd = el('div', 'cmd');
      cmd.appendChild(el('span', 'prompt', '$'));
      cmd.appendChild(el('code', null, command));
      const btn = el('button', 'copy', i18n.t('ui.copy'));
      btn.addEventListener('click', () => copyText(command, btn));
      cmd.appendChild(btn);
      box.appendChild(cmd);
    }
    return box;
  }

  function showRaw(id, payload) {
    const node = $(id);
    if (node) node.textContent = JSON.stringify(payload, null, 2);
  }

  function copyText(text, btn) {
    const done = () => {
      if (!btn) return;
      const old = btn.textContent;
      btn.textContent = i18n.t('ui.copied');
      setTimeout(() => { btn.textContent = old; }, 1400);
    };
    if (navigator.clipboard && navigator.clipboard.writeText) {
      navigator.clipboard.writeText(text).then(done, done);
    } else {
      const ta = document.createElement('textarea');
      ta.value = text;
      document.body.appendChild(ta);
      ta.select();
      try { document.execCommand('copy'); } catch (_) {}
      ta.remove();
      done();
    }
  }

  // Overview

  function renderOverview() {
    const s = state.status;
    const strip = $('statusStrip');
    clear(strip);
    if (!s) return;

    const cells = [
      { label: i18n.t('ui.in_progress'), value: s.currentTotal, hint: i18n.t('ui.someone_is_working_on_it') },
      { label: i18n.t('ui.ready'), value: s.readyCount, hint: i18n.t('ui.dependencies_are_satisfied') },
      {
        label: i18n.t('ui.waiting'),
        value: s.waitingCount != null ? s.waitingCount : '—',
        cls: s.waitingCount > 0 ? 'watch' : '',
        hint: s.waitingCount != null ? i18n.t('ui.waiting_for_a_reply_or_prerequisite_41') : i18n.t('ui.this_awr_version_does_not_report_this'),
      },
      { label: i18n.t('ui.blocked'), value: s.blockedCount, cls: s.blockedCount > 0 ? 'alert' : '', hint: i18n.t('ui.work_is_blocked_inspect_this_first') },
      {
        label: i18n.t('ui.structural_gaps'),
        value: s.gapTotal != null ? s.gapTotal : '—',
        cls: s.gapTotal > 0 ? 'watch' : '',
        hint: s.orgState ? i18n.t('ui.organization_state') + s.orgState : i18n.t('ui.completeness_of_source_structure'),
      },
      { label: 'Revision', value: s.revision != null ? s.revision : '—', dim: true, hint: i18n.t('ui.project_state_revision') },
    ];

    for (const c of cells) {
      const kv = el('div', 'kv');
      kv.appendChild(el('small', null, c.label));
      const b = el('b', [c.dim ? 'dim' : '', c.cls || ''].filter(Boolean).join(' '));
      b.textContent = typeof c.value === 'number' ? group(c.value) : c.value;
      kv.appendChild(b);
      kv.appendChild(el('div', 'kv-hint', c.hint));
      strip.appendChild(kv);
    }

    renderQueueTabs();
    renderQueueList();
    renderGaps(s);
    renderPending(s);

    setText('navWorkCount', String(Object.values(s.queues).reduce((sum, q) => sum + (q.available ? q.total : 0), 0)));
    setText('mcpCmd', `awr-mcp --project ${state.project}`);
    setText('mcpSub', state.mode === 'live' ? i18n.t('ui.this_viewer_uses_cli_agents_use_mcp') : i18n.t('ui.demo_mode'));
  }

  /**
   * Show required, included and budget tokens reported by this compilation.
   * Keep measurements next to the action that produces them. Corpus comparisons
   * are unavailable: AWR does not report corpus size and the browser has no tokenizer.
   */
  function renderPacketSize(ctx) {
    const wrap = $('sizeChart');
    clear(wrap);

    if (!ctx || ctx.total == null) {
      setText('ctxBig', '—');
      setText('ctxCap', i18n.t('ui.not_compiled_yet'));
      setText('sizeSub', '');
      setText('sizeNote', '');
      wrap.appendChild(stateBlock('empty', i18n.t('ui.not_compiled_yet'),
        i18n.t('ui.packet_empty')));
      return;
    }

    const budget = ctx.budget || ctx.total || 1;
    const rows = [
      { label: i18n.t('ui.required_content'), tokens: ctx.requiredTokens, lead: false },
      { label: i18n.t('ui.included_content'), tokens: ctx.total, lead: true },
      { label: i18n.t('ui.budget_limit'), tokens: ctx.budget, lead: false },
    ].filter((r) => Number.isFinite(r.tokens));

    for (const r of rows) {
      const pct = Math.min(100, (r.tokens / budget) * 100);
      const row = el('div', 'cmp-row' + (r.lead ? ' is-lead' : ''));
      const label = el('div', 'cmp-label');
      label.appendChild(document.createTextNode(r.label + ' '));
      label.appendChild(el('span', 'num', group(r.tokens) + ' tokens'));
      row.appendChild(label);
      const track = el('div', 'cmp-track');
      const fill = el('div', 'cmp-fill');
      fill.style.width = pct.toFixed(1) + '%';
      track.appendChild(fill);
      row.appendChild(track);
      wrap.appendChild(row);
    }

    const used = ctx.budget ? Math.round((ctx.total / ctx.budget) * 100) : null;
    setText('ctxBig', group(ctx.total));
    setText('ctxCap', 'tokens' + (ctx.work ? ' · ' + ctx.work : ''));
    setText('sizeSub', used != null ? i18n.t('ui.budget_used', { percent: used }) : '');
    setText('sizeNote', ctx.omissions.length
      ? i18n.t('ui.omitted_note', { count: ctx.omissions.length })
      : i18n.t('ui.nothing_omitted'));
  }

  function renderQueueTabs() {
    const wrap = $('queueTabs');
    clear(wrap);
    for (const q of QUEUES) {
      const n = state.status.queues[q.key].total;
      const chip = el('button', 'chip');
      chip.setAttribute('aria-pressed', String(state.queueTab === q.key));
      chip.appendChild(document.createTextNode(q.label + ' '));
      chip.appendChild(el('b', null, String(n)));
      chip.addEventListener('click', () => {
        state.queueTab = q.key;
        renderQueueTabs();
        renderQueueList();
      });
      wrap.appendChild(chip);
    }
  }

  function renderQueueList() {
    const list = $('queueList');
    clear(list);
    const q = state.status.queues[state.queueTab];
    const meta = queueMeta(state.queueTab);

    setText('queueTitle', meta.label);
    setText('queueSub', q.total > q.items.length ? i18n.t('ui.showing_p0_p1', { p0: q.items.length, p1: q.total }) : i18n.t('ui.total_p0', { p0: q.total }));

    if (!q.available) {
      const li = el('li');
      li.style.gridTemplateColumns = '1fr';
      li.appendChild(stateBlock('empty', i18n.t('ui.this_awr_version_does_not_report_the', { p0: meta.label }),
        i18n.t('ui.this_capability_exists_in_the_source_tree')));
      list.appendChild(li);
      return;
    }

    if (!q.items.length) {
      const li = el('li');
      li.style.gridTemplateColumns = '1fr';
      li.appendChild(stateBlock('empty', i18n.t('ui.the_p0_queue_is_empty', { p0: meta.label }),
        state.queueTab === 'blocked' ? i18n.t('ui.no_work_items_are_blocked') : i18n.t('ui.there_are_currently_no_work_items_in', { p0: meta.label })));
      list.appendChild(li);
      return;
    }

    for (const w of q.items) {
      const li = el('li');
      li.appendChild(el('span', 'dot ' + meta.dot));

      const what = el('span', 'what');
      what.textContent = w.title;
      li.appendChild(what);

      const reason = w.blocker
        || (w.waitTotal ? i18n.t('ui.waiting_for_p0_replies', { p0: w.waitTotal }) : '')
        || (w.codes.length ? w.codes.join(', ') : '')
        || w.nextAction
        || '';
      const meta2 = el('span', 'meta');
      meta2.textContent = w.key + (reason ? ' · ' + reason : '');
      li.appendChild(meta2);

      const age = el('span', 'age' + (state.queueTab === 'blocked' ? ' hot' : ''));
      age.textContent = w.owner || '';
      li.appendChild(age);

      li.style.cursor = 'pointer';
      li.addEventListener('click', () => {
        state.selectedWork = w.key;
        state.overviewWork = w.key;
        state.workFilter = w.queue;
        state.workOffset = 0;
        go('work');
        if (state.workPagination) return loadWorkPage();
        renderWork();
      });
      list.appendChild(li);
    }
  }

  /**
   * status supplies organization.gaps rather than a checkpoint list: missing goals,
   * plans, and work structure are actual reasons an agent cannot start.
   */
  function renderGaps(s) {
    const list = $('cpList');
    clear(list);

    setText('gapSub', s.gapTotal != null && s.gapTotal > s.gaps.length
      ? i18n.t('ui.showing_p0_p1', { p0: s.gaps.length, p1: s.gapTotal })
      : i18n.t('ui.total_p0', { p0: s.gaps.length }));

    if (!s.gaps.length) {
      const li = el('li');
      li.style.gridTemplateColumns = '1fr';
      li.appendChild(stateBlock('empty', i18n.t('ui.no_structural_gaps'),
        i18n.t('ui.the_sources_contain_the_required_goal_plan')));
      list.appendChild(li);
      return;
    }

    for (const g of s.gaps.slice(0, 5)) {
      const li = el('li');
      li.appendChild(el('span', 'dot warn'));
      li.appendChild(el('span', 'what', g.detail || g.code || i18n.t('ui.no_description')));
      li.appendChild(el('span', 'meta', [g.code, g.target].filter(Boolean).join(' · ')));
      li.appendChild(el('span', 'age', ''));
      list.appendChild(li);
    }
  }

  /**
   * Show pending runtime operations only when status supplies them. Release 0.4.0
   * omits this field, so hide the entire panel instead of showing a permanent empty state.
   */
  function renderPending(s) {
    const panel = $('pendingPanel');
    if (!s.pendingOps.length && !s.pendingTotal) {
      panel.hidden = true;
      return;
    }
    panel.hidden = false;
    setText('pendingSub', s.pendingTotal != null && s.pendingTotal > s.pendingOps.length
      ? i18n.t('ui.showing_p0_p1', { p0: s.pendingOps.length, p1: s.pendingTotal })
      : i18n.t('ui.total_p0', { p0: s.pendingOps.length }));

    const list = $('pendingList');
    clear(list);
    for (const op of s.pendingOps.slice(0, 5)) {
      const li = el('li');
      li.appendChild(el('span', 'dot warn'));
      li.appendChild(el('span', 'what', op.code || i18n.t('ui.no_code')));
      li.appendChild(el('span', 'meta', [op.kind, op.id].filter(Boolean).join(' · ')));
      li.appendChild(el('span', 'age', ''));
      list.appendChild(li);
    }
  }

  function isUnsupported(response) {
    return Boolean(response && !response.ok && response.error && response.error.code === 'Unsupported');
  }

  function applyListResponse(field, response, listKey, unsupportedKey) {
    if (isUnsupported(response)) {
      state.unsupported.add(unsupportedKey);
      state[field] = { items: [], loaded: true, error: null, command: response.command, mayHaveMore: false };
      return;
    }
    state.unsupported.delete(unsupportedKey);
    if (response && response.ok) {
      const data = response.data || {};
      state[field] = {
        items: Array.isArray(data[listKey]) ? data[listKey] : [],
        loaded: true,
        error: null,
        command: response.command,
        mayHaveMore: Boolean(data.may_have_more || data.next_cursor),
      };
      return;
    }
    state[field] = {
      items: [],
      loaded: true,
      error: (response && response.error) || { code: 'Error', message: '' },
      command: response && response.command,
      mayHaveMore: false,
    };
  }

  function renderListPanel(panelId, listId, subId, bag, unsupportedKey, emptyTitle, emptyDetail, renderRow) {
    const panel = $(panelId);
    if (!panel) return;
    const list = $(listId);
    if (!bag.loaded || state.unsupported.has(unsupportedKey)) {
      panel.hidden = true;
      return;
    }
    panel.hidden = false;
    setText(subId, bag.mayHaveMore
      ? i18n.t('ui.showing_first_p0', { p0: bag.items.length })
      : i18n.t('ui.total_p0', { p0: bag.items.length }));
    clear(list);
    if (bag.error) {
      const li = el('li');
      li.style.gridTemplateColumns = '1fr';
      li.appendChild(errorBlock(bag.error, bag.command));
      list.appendChild(li);
      return;
    }
    if (!bag.items.length) {
      const li = el('li');
      li.style.gridTemplateColumns = '1fr';
      li.appendChild(stateBlock('empty', emptyTitle, emptyDetail));
      list.appendChild(li);
      return;
    }
    for (const raw of bag.items.slice(0, 20)) {
      list.appendChild(renderRow(raw));
    }
  }

  function renderSessions() {
    const M = FIELD_MAP.session;
    renderListPanel(
      'sessionPanel',
      'sessionList',
      'sessionSub',
      state.sessions,
      'session.list',
      i18n.t('ui.no_active_sessions'),
      i18n.t('ui.no_active_sessions_detail'),
      (raw) => {
        const li = el('li');
        const status = pick(raw, M.status, '');
        li.appendChild(el('span', 'dot' + (status === 'active' ? ' ok' : '')));
        const agent = pick(raw, M.agent, i18n.t('ui.no_title_in_the_source'));
        li.appendChild(el('span', 'what', status ? agent + ' · ' + status : agent));
        const bits = [
          pick(raw, M.work, null) != null ? 'work=' + pick(raw, M.work) : '',
          pick(raw, M.checkpoint, null) != null ? 'checkpoint=' + pick(raw, M.checkpoint) : '',
          [pick(raw, M.provider, ''), pick(raw, M.model, '')].filter(Boolean).join('/'),
        ].filter(Boolean);
        li.appendChild(el('span', 'meta', bits.join(' · ')));
        li.appendChild(el('span', 'age', since(pick(raw, M.startedAt, null))));
        return li;
      }
    );
  }

  function renderEvents() {
    const M = FIELD_MAP.event;
    renderListPanel(
      'eventPanel',
      'eventList',
      'eventSub',
      state.events,
      'event.history',
      i18n.t('ui.no_recent_events'),
      i18n.t('ui.no_recent_events_detail'),
      (raw) => {
        const li = el('li');
        const importance = pick(raw, M.importance, '');
        li.appendChild(el('span', 'dot' + (importance === 'high' ? ' warn' : '')));
        li.appendChild(el('span', 'what', pick(raw, M.summary, i18n.t('ui.no_description'))));
        const bits = [
          pick(raw, M.type, ''),
          pick(raw, M.session, null) != null ? 'session=' + pick(raw, M.session) : '',
          pick(raw, M.work, null) != null ? 'work=' + pick(raw, M.work) : '',
        ].filter(Boolean);
        li.appendChild(el('span', 'meta', bits.join(' · ')));
        const age = el('span', 'age' + (importance === 'high' ? ' hot' : ''));
        age.textContent = since(pick(raw, M.createdAt, null));
        li.appendChild(age);
        return li;
      }
    );
  }

  // Work items

  let workPageGeneration = 0;
  async function loadWorkPage() {
    const generation = ++workPageGeneration;
    detailGuard.invalidate();
    state.workDetail = {};
    state.workPageLoading = true;
    state.workPage = null;
    state.workPageError = null;
    renderWork();
    const response = await callApi(`/api/work-page?queue=${state.workFilter}&offset=${state.workOffset}&limit=${state.workPageSize}`);
    if (generation !== workPageGeneration) return;
    state.workPageLoading = false;
    if (!response.ok || !response.data || !response.data.page) {
      state.workPageError = response.error || { code: 'MissingPage', message: i18n.t('ui.missing_page') };
    } else {
      const page = response.data.page;
      // Removing or completing tasks may invalidate the previous last page.
      if (state.workOffset > 0 && state.workOffset >= page.total) {
        state.workOffset = Math.max(0, Math.ceil(page.total / state.workPageSize) - 1) * state.workPageSize;
        return loadWorkPage();
      }
      detailGuard.invalidate();
      state.status = normStatus(response.data, null);
      state.workPage = page;
      state.raw.overview = { status: response };
      renderOverview();
      showRaw('rawOverviewBody', state.raw.overview);
    }
    return renderWork();
  }

  function renderWork() {
    const s = state.status;
    if (!s) return;

    // Filter chips.
    const filters = $('workFilters');
    clear(filters);
    const options = [{ key: 'all', label: i18n.t('ui.current_queues'), count: Object.values(s.queues).reduce((sum, q) => sum + (q.available ? q.total : 0), 0) }].concat(
      QUEUES.map((q) => ({ key: q.key, label: q.label, count: s.queues[q.key].total }))
    );
    for (const o of options) {
      const chip = el('button', 'chip');
      chip.setAttribute('aria-pressed', String(state.workFilter === o.key));
      chip.appendChild(document.createTextNode(o.label + ' '));
      chip.appendChild(el('b', null, String(o.count)));
      chip.addEventListener('click', () => {
        state.overviewWork = null;
        state.workFilter = o.key;
        state.workOffset = 0;
        if (state.workPagination) loadWorkPage();
        else renderWork();
      });
      filters.appendChild(chip);
    }

    const rows = state.workPagination
      ? (state.workPage ? state.workPage.items.map(normWorkBrief) : [])
      : state.workFilter === 'all' ? s.works : s.works.filter((w) => w.queue === state.workFilter);
    const pager = $('workPagination');
    clear(pager);
    if (state.workPagination) {
      const page = state.workPage;
      const total = page ? page.total : 0;
      const pages = Math.max(1, Math.ceil(total / state.workPageSize));
      const previous = el('button', 'chip', i18n.t('ui.previous_page'));
      previous.disabled = state.workPageLoading || !page || state.workOffset === 0;
      previous.addEventListener('click', () => {
        state.overviewWork = null;
        state.workOffset = Math.max(0, state.workOffset - state.workPageSize);
        return loadWorkPage();
      });
      pager.appendChild(previous);
      pager.appendChild(el('span', 'sub', page ? i18n.t('ui.page_summary', { page: Math.floor(state.workOffset / state.workPageSize) + 1, pages, total }) : state.workPageError ? i18n.t('ui.query_failed') : i18n.t('ui.query_loading')));
      const next = el('button', 'chip', i18n.t('ui.next_page'));
      next.disabled = state.workPageLoading || !page || !page.has_more;
      next.addEventListener('click', () => {
        state.overviewWork = null;
        state.workOffset += state.workPageSize;
        return loadWorkPage();
      });
      pager.appendChild(next);
      const size = el('select');
      size.setAttribute('aria-label', i18n.t('ui.page_size'));
      for (const n of [10, 20, 50, 100]) {
        const option = el('option', null, i18n.t('ui.items_per_page', { count: n })); option.value = String(n); size.appendChild(option);
      }
      size.value = String(state.workPageSize);
      size.disabled = state.workPageLoading;
      size.addEventListener('change', () => {
        state.overviewWork = null;
        state.workPageSize = Number(size.value);
        state.workOffset = 0;
        return loadWorkPage();
      });
      pager.appendChild(size);
    }

    const tbody = $('workRows');
    clear(tbody);
    clear($('workEmpty'));
    const selectedQueues = state.workFilter === 'all'
      ? Object.values(s.queues) : [s.queues[state.workFilter]];
    const omitted = state.workPagination ? 0 : selectedQueues.reduce((sum, q) => sum + q.omitted, 0);
    setText('workSub', omitted ? i18n.t('ui.items_not_loaded', { shown: rows.length, omitted }) : i18n.t('ui.p0_items', { p0: rows.length }));
    if (omitted) {
      $('workEmpty').appendChild(stateBlock('warning', i18n.t('ui.list_incomplete'),
        i18n.t('ui.list_incomplete_help')));
    }

    if (state.workPagination && state.workPageLoading) {
      clear($('workDetail'));
      setText('detailId', state.overviewWork || i18n.t('ui.details'));
      setText('detailStatus', '—');
      $('workEmpty').appendChild(stateBlock('loading', i18n.t('ui.tasks_loading'), ''));
      return;
    }
    if (state.workPagination && state.workPageError) {
      $('workEmpty').appendChild(errorBlock(state.workPageError));
    } else if (!rows.length) {
      $('workEmpty').appendChild(stateBlock('empty', i18n.t('ui.no_work_items_match_this_filter'), i18n.t('ui.try_another_filter')));
    }
    if (state.overviewWork) {
      state.selectedWork = state.overviewWork;
      if (!rows.some((w) => w.key === state.overviewWork) && !state.workPageError) {
        $('workEmpty').appendChild(stateBlock('warning', i18n.t('ui.selected_off_page'),
          i18n.t('ui.selected_off_page_help', { work: state.overviewWork })));
      }
    } else if (!rows.length) {
      clear($('workDetail'));
      setText('detailId', i18n.t('ui.details'));
      setText('detailStatus', '—');
      return;
    } else if (!state.selectedWork || !rows.some((w) => w.key === state.selectedWork)) {
      state.selectedWork = rows[0].key;
    }

    for (const w of rows) {
      const tr = el('tr');
      tr.setAttribute('aria-selected', String(w.key === state.selectedWork));

      const c1 = el('td');
      c1.appendChild(el('span', 'id', w.key));
      tr.appendChild(c1);

      tr.appendChild(el('td', 'wide', w.title));

      const meta = queueMeta(w.queue);
      const c3 = el('td');
      const tagCls = { current: 'info', ready: 'ok', waiting: 'warn', blocked: 'crit' }[w.queue] || '';
      c3.appendChild(el('span', 'tag flat ' + tagCls, meta.label));
      tr.appendChild(c3);

      tr.appendChild(el('td', null, w.rawStatus || w.status || '—'));
      tr.appendChild(el('td', null, w.owner || '—'));
      const claimed = w.raw && Array.isArray(w.raw.claims) && w.raw.claims.length;
      tr.appendChild(el('td', null, claimed ? i18n.t('ui.claimed') : (w.raw && w.raw.ownership_required ? i18n.t('ui.claim_required') : '—')));
      tr.appendChild(el('td', null, w.codes.length ? w.codes.join(', ') : '—'));
      tr.appendChild(el('td', 'num', w.revision != null ? String(w.revision) : '—'));

      tr.addEventListener('click', () => {
        state.overviewWork = null;
        state.selectedWork = w.key;
        return renderWork();
      });
      tbody.appendChild(tr);
    }

    return renderWorkDetail(state.selectedWork);
  }

  async function renderWorkDetail(key) {
    const token = detailGuard.begin(key);
    const box = $('workDetail');
    clear(box);
    setText('detailId', key || i18n.t('ui.details'));
    setText('detailStatus', '—');

    // Cache {detail, raw} together, not just normalized details.
    // Otherwise a cache hit could display A's details while the raw JSON panel
    // still contains B's response.
    let entry = state.workDetail[key];

    if (!entry) {
      box.appendChild(el('div', 'skeleton'));
      let raw = null;
      let detail = null;

      if (state.mode === 'demo') {
        raw = { ok: true, data: window.AWR_DEMO.workShow(key), note: i18n.t('ui.demo_data') };
        if (!detailGuard.isCurrent(token)) return;
        detail = normWorkDetail(raw.data);
      } else {
        const res = await callApi('/api/work?key=' + encodeURIComponent(key));
        // Discard late responses completely: no raw state changes, rendering, or error display.
        // Apply the same rule to failures so they cannot overwrite the current selection.
        if (!detailGuard.isCurrent(token)) return;
        if (!res.ok) {
          // Do not cache failures, but show their raw response for diagnosis.
          state.raw.work = res;
          showRaw('rawWorkBody', res);
          clear(box);
          if (res.error && res.error.code === 'NotFound') {
            box.appendChild(stateBlock('empty', i18n.t('ui.work_missing'), i18n.t('ui.work_missing_help', { work: key })));
          }
          box.appendChild(errorBlock(res.error, res.command));
          return;
        }
        raw = res;
        detail = normWorkDetail(res.data);
      }

      entry = { detail, raw };
      if (detail) state.workDetail[key] = entry;
    }

    if (!detailGuard.isCurrent(token)) return;

    // Restore the raw response on cache hits so it always matches the details.
    state.raw.work = entry.raw;
    showRaw('rawWorkBody', entry.raw);

    const detail = entry.detail;
    clear(box);
    if (!detail) {
      box.appendChild(stateBlock('empty', i18n.t('ui.no_details_for_this_work_item'), i18n.t('ui.it_may_appear_in_a_queue_without')));
      return;
    }

    setText('detailStatus', [queueMeta(detail.queue).label, detail.status].filter(Boolean).join(' · '));

    box.appendChild(el('h3', null, detail.title));

    if (detail.goal) {
      const sec = el('div');
      sec.appendChild(el('h4', null, i18n.t('ui.goal')));
      sec.appendChild(el('p', 'quote', detail.goal));
      box.appendChild(sec);
    }

    if (detail.acceptance.length) {
      const sec = el('div');
      sec.appendChild(el('h4', null, i18n.t('ui.acceptance_criteria_p0_verbatim_from_the_source', { p0: detail.acceptance.length })));
      const ul = el('ul', 'crit-list');
      for (const a of detail.acceptance) {
        const li = el('li');
        li.appendChild(el('span', 'box', '·'));
        li.appendChild(el('span', null, a.criterion));
        ul.appendChild(li);
      }
      sec.appendChild(ul);
      sec.appendChild(el('p', 'figure-note',
        i18n.t('ui.this_view_does_not_record_whether_criteria')));
      box.appendChild(sec);
    }

    const blockText = detail.blocker
      || (detail.waitTotal ? i18n.t('ui.waiting_for_p0_user_replies', { p0: detail.waitTotal }) : null);
    if (blockText) {
      const sec = el('div');
      sec.appendChild(el('h4', null, detail.blocker ? i18n.t('ui.blocker') : i18n.t('ui.waiting_for')));
      sec.appendChild(el('p', 'quote', blockText));
      box.appendChild(sec);
    }

    if (detail.dependsOn.length || detail.missingDeps.length) {
      const sec = el('div');
      sec.appendChild(el('h4', null, i18n.t('ui.dependencies')));
      const parts = detail.dependsOn.map((d) => d.status ? `${d.key}（${d.status}）` : d.key);
      if (parts.length) sec.appendChild(el('p', 'quote', parts.join('、')));
      if (detail.missingDeps.length) {
        sec.appendChild(el('p', 'quote', i18n.t('ui.not_found_in_sources') + detail.missingDeps.join('、')));
      }
      box.appendChild(sec);
    }

    if (detail.claims.length) {
      const sec = el('div');
      sec.appendChild(el('h4', null, i18n.t('ui.who_owns_this_work')));
      const ul = el('ul', 'crit-list');
      for (const c of detail.claims) {
        const li = el('li');
        li.appendChild(el('span', 'box done', '●'));
        const txt = el('span');
        txt.textContent = [c.agent && `agent ${c.agent}`, c.session && `session ${c.session}`]
          .filter(Boolean).join(' · ') || i18n.t('ui.no_identity');
        if (c.expiresAt) {
          txt.appendChild(document.createTextNode(' '));
          txt.appendChild(el('span', 'id', until(c.expiresAt)));
        }
        li.appendChild(txt);
        ul.appendChild(li);
      }
      sec.appendChild(ul);
      sec.appendChild(el('p', 'figure-note',
        i18n.t('ui.a_claim_establishes_awr_ownership_another_session')));
      box.appendChild(sec);
    }

    if (detail.evidence.length || detail.decisions.length) {
      const sec = el('div');
      sec.appendChild(el('h4', null, i18n.t('ui.evidence_and_decisions_p0_evidence_records_p1', { p0: detail.evidence.length, p1: detail.decisions.length })));
      const ul = el('ul', 'crit-list');
      for (const e of detail.evidence.slice(0, 8)) {
        const li = el('li');
        li.appendChild(el('span', 'box done', '✓'));
        li.appendChild(el('span', null,
          [e.external_key || e.key, e.evidence_type || e.kind, e.level].filter(Boolean).join(' · ')));
        ul.appendChild(li);
      }
      for (const d of detail.decisions.slice(0, 8)) {
        const li = el('li');
        li.appendChild(el('span', 'box', '§'));
        li.appendChild(el('span', null, d.title || d.external_key || JSON.stringify(d).slice(0, 80)));
        ul.appendChild(li);
      }
      sec.appendChild(ul);
      box.appendChild(sec);
    }

    if (detail.diagnostics.length || detail.cycles.length) {
      const sec = el('div');
      sec.appendChild(el('h4', null, i18n.t('ui.diagnostics')));
      const codes = detail.diagnostics
        .map((d) => (typeof d === 'string' ? d : d.code))
        .filter(Boolean);
      if (codes.length) sec.appendChild(el('p', 'quote', codes.join('、')));
      if (detail.cycles.length) {
        sec.appendChild(el('p', 'quote', i18n.t('ui.dependency_cycle') + detail.cycles.join(' → ')));
      }
      box.appendChild(sec);
    }

    const sec = el('div');
    sec.appendChild(el('h4', null, i18n.t('ui.next_step')));
    sec.appendChild(el('p', 'quote', detail.nextAction || i18n.t('ui.not_specified_in_the_source')));
    const act = el('div', 'actions');
    const btn = el('button', 'btn', i18n.t('ui.compile_context_for_this_item'));
    btn.addEventListener('click', () => {
      fillWorkSelect(detail);
      $('fWork').value = detail.key;
      if (detail.goal) {
        const m = String(detail.goal).match(/goal#[\w.-]+/);
        if (m) $('fGoal').value = m[0];
      }
      go('context');
      updateCliMirror();
    });
    act.appendChild(btn);
    sec.appendChild(act);
    box.appendChild(sec);
  }

  // Context

  function fillWorkSelect(selectedWork = null) {
    const sel = $('fWork');
    const keep = sel.value;
    const previous = Array.from(sel.children).find(option => option.value === keep);
    const works = new Map((state.status ? state.status.works : []).map(w => [w.key, w]));
    for (const w of state.workPage ? state.workPage.items : []) works.set(w.key, w);
    if (selectedWork) works.set(selectedWork.key, selectedWork);
    clear(sel);
    for (const w of works.values()) {
      const o = el('option', null, `${w.key} — ${w.title}`);
      o.value = w.key;
      sel.appendChild(o);
    }
    // Preserve a context target even after its page is no longer displayed.
    if (previous && !works.has(keep)) sel.appendChild(previous);
    if (keep) sel.value = keep;
    updateCliMirror();
  }

  function updateCliMirror() {
    const work = $('fWork').value;
    const goal = $('fGoal').value.trim();
    const budget = $('fBudget').value;
    const intent = $('fIntent').value.trim();
    let cmd = `awr --project ${state.project} --json context compile --work ${work || '<WORK>'}`;
    if (goal) cmd += ` --goal '${goal}'`;
    cmd += ` --budget ${budget}`;
    if (intent) cmd += ` --intent '${intent}'`;
    setText('cliMirror', cmd);
  }

  async function doCompile() {
    const generation = sourceGeneration;
    const btn = $('compileBtn');
    btn.disabled = true;
    setText('compileHint', i18n.t('ui.compiling'));

    const work = $('fWork').value;
    const goal = $('fGoal').value.trim();
    const budget = Number($('fBudget').value);
    const intent = $('fIntent').value.trim();

    let ctx = null;
    let failure = null;

    if (state.mode === 'demo') {
      await new Promise((r) => setTimeout(r, 260));
      if (generation !== sourceGeneration) return;
      const raw = window.AWR_DEMO.compile(work, budget);
      state.raw.context = { ok: true, data: raw };
      showRaw('rawContextBody', state.raw.context);
      ctx = normContext(raw);
    } else {
      const res = await callApi('/api/context/compile', {
        method: 'POST',
        body: JSON.stringify({ work, goal, budget, intent }),
      });
      if (generation !== sourceGeneration) return;
      state.raw.context = res;
      showRaw('rawContextBody', res);
      if (res.ok) {
        ctx = normContext(res.data);
      } else if (res.data) {
        // An incomplete context exits with code 1 but still provides a report.
        // Render the report alongside its diagnostic.
        ctx = normContext(res.data);
        failure = res;
      } else {
        failure = res;
      }
    }

    btn.disabled = false;
    if (ctx) ctx.work = ctx.work || work;
    state.compile = ctx;

    // Only clear the result when no report is available.
    if (failure && !ctx) {
      // Reset every result panel so previous measurements do not accompany this error.
      renderCompile();
      clear($('breakdown'));
      $('breakdown').appendChild(errorBlock(failure.error, failure.command));
      setText('compileHint', i18n.t('ui.compilation_failed'));
      return;
    }

    renderCompile();

    if (failure) {
      // Preserve the original AWR diagnostic above the incomplete report.
      const cb = $('completeBody');
      const note = el('div', 'state err');
      note.style.padding = '12px 0 0';
      const t = el('div', 'title');
      t.appendChild(el('span', 'errcode', (failure.error && failure.error.code) || 'Error'));
      note.appendChild(t);
      note.appendChild(el('div', 'msg', (failure.error && failure.error.message) || ''));
      cb.appendChild(note);
      setText('compileHint', i18n.t('ui.context_incomplete'));
      return;
    }
    setText('compileHint', ctx.revision != null
      ? i18n.t('ui.revision_p0_sources_unchanged_projection_may_refresh', { p0: ctx.revision })
      : i18n.t('ui.sources_unchanged_projection_may_refresh'));
  }

  function renderCompile() {
    const ctx = state.compile;
    renderPacketSize(ctx);

    const bd = $('breakdown');
    clear(bd);
    if (!ctx) {
      resetOmissions();
      bd.appendChild(stateBlock('empty', i18n.t('ui.not_compiled_yet'), i18n.t('ui.choose_parameters_above_then_select_compile')));
      clear($('completeBody'));
      $('completeBody').appendChild(stateBlock('empty', '—', i18n.t('ui.after_compilation_this_panel_shows_whether_any')));
      setText('packetTotal', '');
      setText('packetNote', '');
      setText('completeSub', '');
      setText('packetPreview', '');
      return;
    }

    const max = Math.max.apply(null, ctx.sections.map((s) => s.count).concat([1]));
    for (const s of ctx.sections) {
      const row = el('div', 'bd-row');
      row.appendChild(el('div', 'bd-name', s.name));
      row.appendChild(el('div', 'bd-val', s.required ? i18n.t('ui.p0_chunks_p1_required', { p0: s.count, p1: s.required }) : i18n.t('ui.p0_chunks', { p0: s.count })));
      const track = el('div', 'bd-track');
      const fill = el('div', 'bd-fill');
      fill.style.width = ((s.count / max) * 100).toFixed(1) + '%';
      track.appendChild(fill);
      row.appendChild(track);
      bd.appendChild(row);
    }

    const over = ctx.budget != null && ctx.total > ctx.budget;
    setText('packetTotal', `${group(ctx.total)} / ${group(ctx.budget)} tokens`);
    // AWR provides a total token count and per-chunk section/required fields only.
    // Count chunks here instead of inventing per-section token estimates.
    setText('packetNote', over
      ? i18n.t('ui.the_total_exceeds_the_budget_awr_omits')
      : i18n.t('ui.p0_chunks_in_total_bars_measure_chunks', { p0: ctx.chunkTotal }));

    // Completeness.
    const cb = $('completeBody');
    clear(cb);
    const ok = ctx.complete !== false;
    setText('completeSub', ok ? i18n.t('ui.complete') : i18n.t('ui.omissions_present'));

    const head = el('div', 'loops');
    const li = el('li');
    li.appendChild(el('span', 'dot ' + (ok ? 'ok' : 'warn')));
    li.appendChild(el('span', 'what', ctx.statusText || (ok ? i18n.t('ui.context_complete') : i18n.t('ui.context_incomplete'))));
    li.appendChild(el('span', 'meta', ctx.requiredTokens != null ? i18n.t('ui.required_content_p0_tokens', { p0: group(ctx.requiredTokens) }) : ''));
    head.appendChild(li);
    cb.appendChild(head);

    // Show each independently assessed dimension so missing facts are visible.
    if (ctx.dimensions.length) {
      const chips = el('div', 'chips');
      chips.style.marginTop = '14px';
      for (const d of ctx.dimensions) {
        const chip = el('span', 'tag flat ' + (d.ok ? 'ok' : 'crit'), d.label);
        chips.appendChild(chip);
      }
      cb.appendChild(chips);
    }

    // Evidence gaps identify acceptance criteria without supporting evidence.
    if (ctx.evidenceGaps.length) {
      const h = el('p', 'figure-note');
      h.style.marginBottom = '6px';
      h.textContent = i18n.t('ui.p0_evidence_gaps_each_acceptance_criterion_needs', { p0: ctx.evidenceGaps.length });
      cb.appendChild(h);
      const ul = el('ul', 'crit-list');
      for (const g of ctx.evidenceGaps.slice(0, 6)) {
        const item = el('li');
        item.appendChild(el('span', 'box', '!'));
        item.appendChild(el('span', null,
          typeof g === 'string' ? g : [g.reference, g.reason || g.code].filter(Boolean).join('：')));
        ul.appendChild(item);
      }
      cb.appendChild(ul);
    }

    if (ctx.unresolvedDeps.length) {
      const ul = el('ul', 'crit-list');
      ul.style.marginTop = '10px';
      for (const d of ctx.unresolvedDeps.slice(0, 6)) {
        const item = el('li');
        item.appendChild(el('span', 'box', '⛔'));
        item.appendChild(el('span', null, i18n.t('ui.unresolved_dependency') + (typeof d === 'string' ? d : (d.external_key || JSON.stringify(d)))));
        ul.appendChild(item);
      }
      cb.appendChild(ul);
    }

    if (ctx.issues.length) {
      const ul = el('ul', 'crit-list');
      ul.style.marginTop = '10px';
      for (const i of ctx.issues.slice(0, 6)) {
        const item = el('li');
        item.appendChild(el('span', 'box', '!'));
        item.appendChild(el('span', null, typeof i === 'string' ? i : (i.detail || i.code || JSON.stringify(i))));
        ul.appendChild(item);
      }
      cb.appendChild(ul);
    }

    renderOmissions(ctx, cb);

    setText('packetPreview', ctx.rendered || i18n.t('ui.no_rendered_text_was_returned'));
  }

  /**
   * Group omitted chunks by section so internal IDs do not obscure missing facts.
   * Keep the original IDs in an expandable section for inspection.
   */
  /** Clear collapsed content as well as hiding it to prevent stale IDs reappearing. */
  function resetOmissions() {
    const box = $('omittedBox');
    if (box) {
      box.hidden = true;
      box.open = false;
    }
    setText('omittedSummary', '');
    setText('omittedList', '');
  }

  function renderOmissions(ctx, cb) {
    const box = $('omittedBox');
    if (!ctx.omissions.length) {
      resetOmissions();
      return;
    }

    const bySection = new Map();
    const reasons = new Set();
    for (const o of ctx.omissions) {
      const name = o.section || i18n.t('ui.unsectioned');
      bySection.set(name, (bySection.get(name) || 0) + 1);
      if (o.reason) reasons.add(o.reason);
    }

    const line = el('p', 'figure-note');
    line.style.marginTop = '14px';
    const parts = [...bySection.entries()]
      .sort((a, b) => b[1] - a[1])
      .map(([name, n]) => `${name} ${n}`);
    line.appendChild(el('b', null, i18n.t('ui.omitted_count', { count: ctx.omissions.length })));
    line.appendChild(document.createTextNode(': ' + parts.join(' · ')));
    if (reasons.size) {
      line.appendChild(document.createTextNode(i18n.t('ui.omission_reasons', { reasons: [...reasons].join(', ') })));
    }
    cb.appendChild(line);

    const tip = el('p', 'figure-note', i18n.t('ui.increase_the_budget_and_compile_again_to'));
    tip.style.marginTop = '4px';
    cb.appendChild(tip);

    if (box) {
      box.hidden = false;
      box.open = false;
      setText('omittedSummary', i18n.t('ui.omitted_ids', { count: ctx.omissions.length }));
      setText(
        'omittedList',
        ctx.omissions.map((o) => `${o.section || '—'}\t${o.key || o.detail}`).join('\n')
      );
    }
  }

  // Indexed sources

  function renderSources() {
    const data = state.sources;
    const tbody = $('srcRows');
    clear(tbody);
    clear($('srcEmpty'));
    if (!data) return;

    setText('srcCmd', `awr --project ${state.project} --json intake inspect`);

    const files = data.files;
    const stale = files.filter((f) => f.state !== 'fresh' && f.state !== 'indexed').length;

    setText('srcTitle', i18n.t('ui.p0_sources', { p0: files.length }));
    setText('srcSub', [
      data.summary ? Object.keys(data.summary).map((k) => `${data.summary[k]} ${k}`).join(' · ') : '',
      stale ? i18n.t('ui.p0_stale', { p0: stale }) : '',
      data.issues.length ? i18n.t('ui.p0_issues', { p0: data.issues.length }) : '',
    ].filter(Boolean).join(' · '));
    setText('navSourceCount', String(files.length || ''));

    if (!files.length) {
      $('srcEmpty').appendChild(stateBlock('empty', i18n.t('ui.the_manifest_matched_no_files'),
        i18n.t('ui.check_the_sources_paths_in_project_toml'),
        `awr --project ${state.project} intake inspect`));
      return;
    }

    const tagFor = { fresh: 'ok', indexed: 'ok', stale: 'warn', drift: 'warn', new: 'info', rejected: 'crit' };

    for (const f of files) {
      const tr = el('tr');
      tr.appendChild(el('td', 'wide', f.path));
      tr.appendChild(el('td', null, f.kind));
      tr.appendChild(el('td', null, f.role || '—'));

      const c4 = el('td');
      c4.appendChild(el('span', 'tag flat ' + (tagFor[f.state] || 'warn'), f.state));
      tr.appendChild(c4);

      tr.appendChild(el('td', 'num', f.revision != null ? String(f.revision) : '—'));
      tbody.appendChild(tr);

      if (f.rejection) {
        const exp = el('tr', 'expando');
        exp.hidden = true;
        const td = el('td');
        td.colSpan = 5;
        const r = f.rejection;
        const loc = r.location
          ? (r.location.line != null ? i18n.t('ui.line_p0', { p0: r.location.line }) + (r.location.column != null ? i18n.t('ui.column_p0', { p0: r.location.column }) : '') : i18n.t('ui.location_unavailable'))
          : i18n.t('ui.location_unavailable');
        td.appendChild(el('div', null, i18n.t('ui.rule_p0', { p0: r.rule || '—' })));
        td.appendChild(el('div', null, i18n.t('ui.location_p0', { p0: loc })));
        td.appendChild(el('div', null, i18n.t('ui.suggested_repair_p0', { p0: r.repair || '—' })));
        const note = el('p', 'figure-note', i18n.t('ui.awr_does_not_echo_the_matched_value'));
        td.appendChild(note);
        exp.appendChild(td);
        tbody.appendChild(exp);

        tr.addEventListener('click', () => { exp.hidden = !exp.hidden; });
        tr.title = i18n.t('ui.select_to_inspect_the_rejection_reason');
      }
    }
  }

  async function doReindex() {
    const btn = $('reindexBtn');

    // This is the only UI action that changes AWR state; require explicit confirmation.
    const okToRun = window.confirm(
      i18n.t('ui.reindexing_refreshes_the_source_projection_and_advances') +
      i18n.t('ui.it_does_not_change_your_markdown_yaml')
    );
    if (!okToRun) return;

    btn.disabled = true;
    const old = btn.textContent;
    btn.textContent = i18n.t('ui.indexing');

    if (state.mode === 'demo') {
      await new Promise((r) => setTimeout(r, 500));
      btn.disabled = false;
      btn.textContent = old;
      clear($('srcEmpty'));
      $('srcEmpty').appendChild(stateBlock('empty', i18n.t('ui.demo_mode_does_not_reindex'),
        i18n.t('ui.when_connected_to_a_real_project_this'),
        i18n.t('ui.awr_project_source_reindex')));
      return;
    }

    const res = await callApi('/api/source/reindex', { method: 'POST' });
    btn.disabled = false;
    btn.textContent = old;

    if (!res.ok) {
      clear($('srcEmpty'));
      $('srcEmpty').appendChild(errorBlock(res.error, res.command));
      return;
    }
    // Reindexing advances the revision, so invalidate all caches and reload.
    detailGuard.invalidate();
    state.workDetail = {};
    state.compile = null;
    await loadAll();
  }

  // Loading

  function resetProjectData() {
    // Work keys, options and caches belong to one source; demo is a separate source.
    ++sourceGeneration;
    ++workPageGeneration;
    detailGuard.invalidate();
    state.overviewWork = null;
    state.selectedWork = null;
    state.workFilter = 'all';
    state.workOffset = 0;
    state.workPage = null;
    state.workPageLoading = false;
    state.workPageError = null;
    state.workDetail = {};
    state.status = null;
    state.sources = null;
    state.sessions = { items: [], loaded: false, error: null, command: null, mayHaveMore: false };
    state.events = { items: [], loaded: false, error: null, command: null, mayHaveMore: false };
    state.unsupported = new Set();
    state.compile = null;
    state.raw = {};
    for (const id of ['fWork', 'workRows', 'workFilters', 'workPagination', 'workEmpty', 'workDetail',
      'statusStrip', 'queueList', 'queueTabs', 'cpList', 'pendingList', 'sessionList', 'eventList',
      'rawWorkBody', 'rawContextBody', 'rawOverviewBody', 'rawSourcesBody']) clear($(id));
    for (const id of ['navWorkCount', 'navSourceCount', 'workSub', 'queueSub', 'gapSub', 'pendingSub',
      'sessionSub', 'eventSub', 'srcTitle', 'srcSub', 'srcCmd', 'compileHint']) setText(id, '');
    if ($('sessionPanel')) $('sessionPanel').hidden = true;
    if ($('eventPanel')) $('eventPanel').hidden = true;
    $('fGoal').value = '';
    $('fIntent').value = '';
    $('compileBtn').disabled = false;
    setText('detailId', i18n.t('ui.details'));
    setText('detailStatus', '—');
    renderSources();
    renderCompile();
    updateCliMirror();
  }

  let loadGeneration = 0;
  async function loadAll() {
    const generation = ++loadGeneration;
    const previousMode = state.mode;
    const previousProject = state.project;
    ++workPageGeneration;
    detailGuard.invalidate();
    state.workDetail = {};
    state.workPage = null;
    const health = await callApi('/api/health');
    if (generation !== loadGeneration) return;
    if (health.ok) {
      state.mode = health.data.mode;
      state.teamOnly = health.data.teamOnly === true;
      // In demo mode, show the sample project name rather than this tool's directory;
      // the latter would misleadingly suggest the displayed data came from that path.
      state.project = state.mode === 'demo' ? '.local/demo' : (health.data.project || '.');
      state.reason = health.data.reason;
      setText('verTag', health.data.awrVersion || 'v0.4.0');
    } else {
      state.mode = 'demo';
      state.reason = i18n.t('ui.cannot_reach_the_local_bridge_node_server');
      state.project = '.local/demo';
    }

    if (state.mode !== previousMode || state.project !== previousProject) resetProjectData();
    state.workPagination = state.mode !== 'demo';
    setText('projPath', state.project);
    configureNavigation();
    renderModeUi();

    if (state.teamOnly) {
      go('team', false);
      if (teamWeb) await teamWeb.refresh();
      return;
    }

    if (state.mode === 'demo') {
      state.status = normStatus(window.AWR_DEMO.status, null);
      state.sources = normSources(window.AWR_DEMO.sources);
      state.sessions = {
        items: window.AWR_DEMO.sessions || [],
        loaded: true,
        error: null,
        command: null,
        mayHaveMore: false,
      };
      state.events = {
        items: window.AWR_DEMO.events || [],
        loaded: true,
        error: null,
        command: null,
        mayHaveMore: false,
      };
      state.raw.overview = {
        ok: true,
        data: {
          status: window.AWR_DEMO.status,
          sessions: window.AWR_DEMO.sessions,
          events: window.AWR_DEMO.events,
        },
        note: i18n.t('ui.demo_data'),
      };
      state.raw.sources = { ok: true, data: window.AWR_DEMO.sources, note: i18n.t('ui.demo_data') };
    } else {
      // Use summaries for Overview and fetch work pages on demand.
      const [st, src, sess, ev] = await Promise.all([
        callApi('/api/status'),
        callApi('/api/sources'),
        callApi('/api/sessions?limit=20'),
        callApi('/api/events?limit=20'),
      ]);
      if (generation !== loadGeneration) return;
      applyListResponse('sessions', sess, 'sessions', 'session.list');
      applyListResponse('events', ev, 'events', 'event.history');
      state.raw.overview = { status: st, sessions: sess, events: ev };
      state.raw.sources = src;

      if (st.ok) {
        state.status = normStatus(st.data, null);
      } else {
        state.status = null;
        clear($('statusStrip'));
        $('statusStrip').appendChild(errorBlock(st.error, st.command));
      }
      state.sources = src.ok ? normSources(src.data) : null;
      if (!src.ok) {
        clear($('srcEmpty'));
        $('srcEmpty').appendChild(errorBlock(src.error, src.command));
      }
    }

    showRaw('rawOverviewBody', state.raw.overview);
    showRaw('rawSourcesBody', state.raw.sources);

    if (state.status) {
      renderOverview();
      if (state.workPagination) await loadWorkPage();
      else await renderWork();
      if (generation !== loadGeneration) return;
      fillWorkSelect();
    }
    if (state.sources) renderSources();
    renderSessions();
    renderEvents();
    renderCompile();

    // The freshness badge depends on status; render it again after status arrives.
    renderModeUi();
    if (state.view === 'team' && teamWeb) await teamWeb.refresh();
  }

  function configureNavigation() {
    for (const link of document.querySelectorAll('.rail a')) {
      link.hidden = Boolean(state.teamOnly && link.dataset.view !== 'team');
    }
    $('projectPicker').hidden = Boolean(state.teamOnly);
    $('btnGuide').hidden = Boolean(state.teamOnly);
    setText('navHeading', i18n.t(state.teamOnly ? 'ui.team_web' : 'ui.runtime'));
    setText('footBrand', state.teamOnly ? 'AWR Team' : i18n.t('ui.awr_inspector_local_project_viewer'));
  }

  function renderModeUi() {
    const demo = state.mode === 'demo';
    $('tagDemo').hidden = !demo;
    $('tagBackend').hidden = demo;
    if (!demo) $('tagBackend').textContent = i18n.t(state.teamOnly ? 'ui.team_service_entry' : 'ui.cli_connected');

    const fresh = $('tagFresh');
    if (state.status && state.status.lastIndexed) {
      fresh.hidden = false;
      const drift = state.status.driftCount;
      fresh.className = 'tag ' + (drift ? 'warn' : 'ok live');
      fresh.textContent = drift ? i18n.t('ui.p0_files_have_drifted', { p0: drift }) : i18n.t('ui.indexed_p0', { p0: since(state.status.lastIndexed) });
    } else {
      fresh.hidden = true;
    }

    setText('footMode', demo ? i18n.t('ui.demo_mode_synthetic_data') : i18n.t('ui.live_data_p0', { p0: state.project }));
    if (state.teamOnly) setText('footMode', i18n.t('ui.team_live_data'));

    const banner = $('modeBanner');
    if (demo && !sessionStorage.getItem('awr.banner.hidden')) {
      banner.hidden = false;
      banner.className = 'banner';
      const text = $('modeBannerText');
      clear(text);
      const b = el('b', null, i18n.t('ui.you_are_viewing_demo_data'));
      text.appendChild(b);
      text.appendChild(document.createTextNode(' ' + (state.reason || '') + i18n.t('ui.to_view_your_project_install_awr_and')));
      text.appendChild(el('code', null, i18n.t('ui.node_server_js_project_path_to_project')));
      text.appendChild(document.createTextNode(i18n.t('ui.the_interface_works_the_same_way_explore')));
    } else {
      banner.hidden = true;
    }
  }


  // Mainline navigation (WS-042, read-only)
  async function loadMainline() {
    const params = new URLSearchParams();
    const workstream = ($('navWorkstream') || {}).value || '';
    const goal = ($('navGoal') || {}).value || '';
    const milestone = ($('navMilestone') || {}).value || '';
    const works = (($('navWorkKeys') || {}).value || '')
      .split(',')
      .map((s) => s.trim())
      .filter(Boolean);
    if (workstream) params.set('workstream', workstream);
    if (goal) params.set('goal', goal);
    if (milestone) params.set('milestone', milestone);
    for (const work of works) params.append('work', work);
    const path = '/api/mainline-nav' + (params.toString() ? '?' + params.toString() : '');
    const response = await callApi(path);
    state.raw.mainline = response;
    renderMainline(response);
    showRaw('rawMainlineBody', response);
  }

  function renderMainline(response) {
    // The bridge returns { ok, command, data: <awr nav json> }. Accounting and
    // blockers live on data, not on result or the envelope itself.
    const payload = response && response.data && typeof response.data === 'object'
      ? response.data
      : (response && response.result ? response.result : response);
    const acc = (payload && payload.accounting) || {};
    const accBody = $('navAccBody');
    const accSub = $('navAccSub');
    if (accBody) {
      clear(accBody);
      if (acc.available) {
        if (accSub) accSub.textContent = i18n.t('ui.authoritative_accounting');
        accBody.appendChild(el('p', null, i18n.t('ui.required_count_p0', { p0: acc.required_count })));
        accBody.appendChild(el('p', null, i18n.t('ui.stages_independent_not_goal_rate')));
        const stages = ['planned', 'implemented', 'verified', 'merged', 'released'];
        const ul = el('ul', { class: 'loops' });
        for (const stage of stages) {
          const s = acc[stage] || {};
          ul.appendChild(el('li', null, `${stage}: recorded=${s.recorded ?? 0} not_met=${s.not_met ?? 0} unknown=${s.unknown ?? 0}`));
        }
        accBody.appendChild(ul);
      } else {
        if (accSub) accSub.textContent = i18n.t('ui.accounting_unavailable');
        accBody.appendChild(el('p', null, acc.reason || acc.detail || i18n.t('ui.attach_ws040_accounting')));
      }
      const blockers = (payload && payload.blockers) || [];
      accBody.appendChild(el('h3', null, i18n.t('ui.blockers')));
      if (!blockers.length) {
        accBody.appendChild(el('p', null, i18n.t('ui.no_blockers_in_scope')));
      } else {
        const ul = el('ul', { class: 'loops' });
        for (const b of blockers) {
          ul.appendChild(el('li', null, `${b.work}: ${b.blocker}`));
        }
        accBody.appendChild(ul);
      }
      const cross = (payload && payload.cross_dependencies) || [];
      accBody.appendChild(el('h3', null, i18n.t('ui.cross_dependencies')));
      if (!cross.length) {
        accBody.appendChild(el('p', null, i18n.t('ui.no_cross_workstream_edges')));
      } else {
        const ul = el('ul', { class: 'loops' });
        for (const e of cross) {
          ul.appendChild(el('li', null, `${e.from} → ${e.to} · ${e.outcome}`));
        }
        accBody.appendChild(ul);
      }
    }
    const graphBody = $('navGraphBody');
    if (graphBody) {
      clear(graphBody);
      const graph = (payload && payload.mainline_graph) || {};
      const nodes = graph.nodes || [];
      const edges = graph.edges || [];
      graphBody.appendChild(el('p', { class: 'sub' }, i18n.t('ui.edge_basis_concrete_outcomes')));
      const nodeList = el('ul', { class: 'loops' });
      for (const n of nodes) {
        const li = el('li');
        li.appendChild(el('strong', null, n.work_key || ''));
        li.appendChild(document.createTextNode(` [${n.status || ''}] ${n.title || ''}`));
        if (n.person_responsibility) {
          li.appendChild(el('div', { class: 'sub' }, i18n.t('ui.person_p0', { p0: n.person_responsibility })));
        }
        if (n.agent_execution) {
          li.appendChild(el('div', { class: 'sub' }, i18n.t('ui.agent_p0', { p0: JSON.stringify(n.agent_execution) })));
        }
        if (n.stage_acceptance_window) {
          li.appendChild(el('div', { class: 'sub' }, i18n.t('ui.acceptance_window_p0', { p0: JSON.stringify(n.stage_acceptance_window) })));
        }
        for (const w of n.explainable_waits || []) {
          li.appendChild(el('div', { class: 'sub' }, `${w.kind}: ${w.summary} → ${w.release_condition}`));
        }
        nodeList.appendChild(li);
      }
      graphBody.appendChild(nodeList);
      const edgeList = el('ul', { class: 'loops' });
      for (const e of edges) {
        edgeList.appendChild(el('li', null, `${e.from} → ${e.to} · ${e.outcome}`));
      }
      graphBody.appendChild(el('h3', null, i18n.t('ui.outcome_edges')));
      graphBody.appendChild(edgeList);
    }
    const guideBody = $('navGuideBody');
    if (guideBody) {
      clear(guideBody);
      const g = (payload && payload.guidance) || {};
      guideBody.appendChild(el('p', null, `${i18n.t('ui.when')}: ${g.when || ''}`));
      guideBody.appendChild(el('p', null, `${i18n.t('ui.basis')}: ${g.basis || ''}`));
      guideBody.appendChild(el('p', null, `${i18n.t('ui.next_action')}: ${g.next_action || ''}`));
      guideBody.appendChild(el('p', null, `${i18n.t('ui.recheck')}: ${g.recheck || ''}`));
      guideBody.appendChild(el('p', { class: 'sub' }, i18n.t('ui.ws044_writes_deferred')));
    }
  }


  // Navigation

  // Team Web collaboration loop handle (WS-044); initialized in boot().
  let teamWeb = null;
  // Project map controller; created in boot() when its scripts are loaded.
  let projectMap = null;

  const VIEWS = ['overview', 'work', 'context', 'mainline', 'map', 'sources', 'team'];

  function go(view, refresh = true) {
    if (state.teamOnly) view = 'team';
    if (VIEWS.indexOf(view) < 0) view = 'overview';
    state.view = view;
    for (const v of VIEWS) $('view-' + v).hidden = v !== view;
    for (const a of document.querySelectorAll('.rail a')) {
      a.setAttribute('aria-current', String(a.dataset.view === view));
    }
    history.replaceState(null, '', '#' + view);
    window.scrollTo({ top: 0 });
    if (refresh && view === 'mainline' && !state.raw.mainline) {
      loadMainline().catch((e) => errorBlock(e, 'awr nav'));
    }
    if (refresh && view === 'team' && window.AWR_TEAM_WEB && teamWeb) {
      teamWeb.refresh().catch((e) => errorBlock(e, 'team web'));
    }
    if (refresh && view === 'map' && projectMap) projectMap.show();
  }

  // Getting-started tour

  const TOUR = [
    {
      title: i18n.t('ui.what_this_tool_does'),
      html: [
        i18n.t('ui.each_new_coding_agent_session_needs_to'),
        i18n.t('ui.awr_inspector_gives_people_a_view_of'),
        i18n.t('ui.source_filesawr_indexcontext_packet'),
        i18n.t('ui.benchmark_explanation'),
      ].join(''),
    },
    {
      title: i18n.t('ui.first_stop_overview'),
      html: [
        i18n.t('ui.four_queues_describe_work_in_progress_ready'),
        i18n.t('ui.start_with_the_last_two_to_see'),
      ].join(''),
    },
    {
      title: i18n.t('ui.second_stop_work_items'),
      html: [
        i18n.t('ui.inspect_a_work_item_s_goal_acceptance'),
        i18n.t('ui.acceptance_criteria_are_copied_verbatim_from_your'),
      ].join(''),
    },
    {
      title: i18n.t('ui.third_stop_context'),
      html: [
        i18n.t('ui.compile_a_context_packet_and_inspect_its'),
        i18n.t('ui.compilation_does_not_change_authoritative_sources_or'),
      ].join(''),
    },
    {
      title: i18n.t('ui.finally_run_any_command_yourself'),
      html: [
        i18n.t('ui.each_box_shows_the_command_executed_by'),
        i18n.t('ui.need_a_definition_open_the_glossary_at'),
      ].join(''),
    },
  ];

  let tourStep = 0;

  function openTour(step) {
    tourStep = step || 0;
    renderTour();
    $('tour').hidden = false;
  }

  function renderTour() {
    const s = TOUR[tourStep];
    setText('tourTitle', `${tourStep + 1}/${TOUR.length} · ${s.title}`);
    $('tourBody').innerHTML = s.html;
    setText('tourNext', tourStep === TOUR.length - 1 ? i18n.t('ui.get_started') : i18n.t('ui.next_step'));
    const dots = $('tourDots');
    clear(dots);
    TOUR.forEach((_, i) => {
      const d = el('i');
      if (i === tourStep) d.className = 'on';
      dots.appendChild(d);
    });
  }

  function closeTour() {
    $('tour').hidden = true;
    try { localStorage.setItem('awr.tour.seen', '1'); } catch (_) {}
  }

  // Startup

  function wire() {
    for (const a of document.querySelectorAll('.rail a')) {
      a.addEventListener('click', (e) => {
        e.preventDefault();
        go(a.dataset.view);
      });
    }

    // Contextual help buttons.
    for (const b of document.querySelectorAll('.why')) {
      b.addEventListener('click', () => {
        const note = document.querySelector(`[data-note="${b.dataset.why}"]`);
        if (note) note.hidden = !note.hidden;
      });
    }

    // Copy buttons.
    for (const b of document.querySelectorAll('.copy[data-copy-target]')) {
      b.addEventListener('click', () => {
        const target = $(b.dataset.copyTarget);
        if (target) copyText(target.textContent, b);
      });
    }

    const navLoadBtn = $('navLoadBtn');
    if (navLoadBtn) {
      navLoadBtn.addEventListener('click', () => loadMainline().catch((e) => errorBlock(e, 'awr nav')));
    }

    $('btnRefresh').addEventListener('click', async () => {
      const b = $('btnRefresh');
      b.classList.add('spin');
      try {
        if (state.view === 'team' && teamWeb) await teamWeb.refresh();
        else if (state.view === 'map' && projectMap) await projectMap.refresh();
        else await loadAll();
      } finally { b.classList.remove('spin'); }
    });

    $('btnGuide').addEventListener('click', () => openTour(0));
    $('tourNext').addEventListener('click', () => {
      if (tourStep === TOUR.length - 1) closeTour();
      else { tourStep++; renderTour(); }
    });
    $('tourSkip').addEventListener('click', closeTour);
    $('tour').addEventListener('click', (e) => { if (e.target === $('tour')) closeTour(); });

    $('btnGloss').addEventListener('click', () => { $('glossary').hidden = false; });
    $('glossClose').addEventListener('click', () => { $('glossary').hidden = true; });
    $('glossary').addEventListener('click', (e) => { if (e.target === $('glossary')) $('glossary').hidden = true; });

    document.addEventListener('keydown', (e) => {
      if (e.key === 'Escape') {
        $('tour').hidden = true;
        $('glossary').hidden = true;
      }
    });

    $('modeBannerClose').addEventListener('click', () => {
      $('modeBanner').hidden = true;
      try { sessionStorage.setItem('awr.banner.hidden', '1'); } catch (_) {}
    });

    $('btnTheme').addEventListener('click', () => {
      const cur = document.documentElement.getAttribute('data-theme');
      const isDark = cur ? cur === 'dark' : matchMedia('(prefers-color-scheme: dark)').matches;
      const next = isDark ? 'light' : 'dark';
      document.documentElement.setAttribute('data-theme', next);
      try { localStorage.setItem('awr.theme', next); } catch (_) {}
    });

    for (const id of ['fWork', 'fGoal', 'fBudget', 'fIntent']) {
      $(id).addEventListener('input', updateCliMirror);
      $(id).addEventListener('change', updateCliMirror);
    }
    $('compileBtn').addEventListener('click', doCompile);
    $('reindexBtn').addEventListener('click', doReindex);
  }

  function restoreTheme() {
    try {
      const saved = localStorage.getItem('awr.theme');
      if (saved) document.documentElement.setAttribute('data-theme', saved);
    } catch (_) {}
  }

  async function boot() {
    restoreTheme();
    wire();
    if (window.AWR_TEAM_WEB && typeof window.AWR_TEAM_WEB.createTeamWeb === 'function') {
      teamWeb = window.AWR_TEAM_WEB.createTeamWeb({
        i18n: i18n,
        $: $,
        callApi: callApi,
      });
    }
    if (window.AWR_PROJECT_MAP && window.AWR_PROJECT_MAP.ui) {
      projectMap = window.AWR_PROJECT_MAP.ui.createProjectMap({ i18n: i18n, $: $, callApi: callApi, fetchSource: fetchAsset });
    }
    go((location.hash || '#overview').slice(1), false);
    await loadAll();
    if (state.view === 'mainline') await loadMainline();
    if (state.view === 'map' && projectMap) await projectMap.show();

    let seen = null;
    try { seen = localStorage.getItem('awr.tour.seen'); } catch (_) {}
    if (!seen && !state.teamOnly) openTour(0);
  }

  if (typeof document !== 'undefined') {
    document.addEventListener('DOMContentLoaded', boot);
  }

  // Test exports; browsers have no module object and skip this block.
  if (typeof module !== 'undefined' && module.exports) {
    // teamWeb may be null in non-browser fixtures
    module.exports = {
      teamWeb: () => teamWeb,
      createGenerationGuard, state, detailGuard, renderWorkDetail, normStatus, renderWork, loadWorkPage,
      renderPacketSize, doCompile, renderQueueList, fillWorkSelect, loadAll,
      renderSessions, renderEvents, applyListResponse,
    };
  }
})();
