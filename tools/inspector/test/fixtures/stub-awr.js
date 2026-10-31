#!/usr/bin/env node
/**
 * A fake awr executable for tests.
 *
 * Matches the AWR 0.4.0 argument signatures and injects boundary cases:
 * multibyte output, oversized output, slow commands, and JSON errors on stderr.
 *
 * Environment variables:
 *   STUB_MODE=multibyte   Write valid UTF-8 JSON one byte at a time.
 *   STUB_MODE=huge        Produce more output than the bridge limit.
 *   STUB_MODE=slow        Sleep beyond the command timeout.
 *   STUB_MODE=stderrjson  Write a JSON error to stderr and exit nonzero.
 *   STUB_ARGV_OUT=<path>  Save the exact argv to this file for assertions.
 */

'use strict';

const fs = require('fs');

const argv = process.argv.slice(2);
const mode = process.env.STUB_MODE || '';

if (process.env.STUB_ARGV_OUT) {
  fs.appendFileSync(process.env.STUB_ARGV_OUT, JSON.stringify(argv) + '\n');
}

if (argv.includes('--version')) {
  process.stdout.write('awr 0.4.0-stub\n');
  process.exit(0);
}

const joined = argv.join(' ');

function emit(obj) {
  process.stdout.write(JSON.stringify(obj));
  process.exit(0);
}

if (mode === 'stderrjson') {
  process.stderr.write(JSON.stringify({ code: 'SourceStale', message: 'a source changed' }));
  process.exit(3);
}

if (mode === 'slowwrite') {
  // Outlive the write timeout to verify slots follow child lifetimes, not responses.
  setTimeout(() => process.exit(0), 30 * 1000);
  setInterval(() => {}, 1000);
  return;
}

if (mode === 'hugewrite') {
  // Overflow write output after a delay to exercise the timeout path; do not kill the writer.
  const delay = Number(process.env.STUB_WRITE_DELAY_MS || 0);
  setTimeout(() => {
    const block = Buffer.alloc(1024 * 1024, 0x61);
    for (let i = 0; i < 12; i++) {
      try {
        fs.writeSync(1, block);
      } catch (e) {
        if (e.code === 'EPIPE') break;
        throw e;
      }
    }
    // Stay alive longer so the test can verify that SIGKILL was not sent.
    setTimeout(() => process.exit(0), 3000);
  }, delay);
  setInterval(() => {}, 1000);
  return;
}

if (mode === 'incomplete' && joined.includes('context compile')) {
  // Reproduce context compile: incomplete context exits with code 1
  // while stdout still contains the report.
  fs.writeSync(1,
    JSON.stringify({
      ok: false,
      project_revision: 9,
      error: { code: 'ContextIncomplete', message: 'context incomplete: L1 has required gaps' },
      completeness: {
        complete: false,
        status: 'CONTEXT INCOMPLETE',
        project_revision: 9,
        rules_complete: false,
        acceptance_complete: true,
        issues: [{ code: 'hard_rule_unresolved', field: 'rules_complete' }],
        evidence_gaps: [],
        unresolved_required_dependencies: [],
      },
      work_context: {
        rendered_context: '# 不完整但仍然有内容',
        token_estimate: 120,
        required_tokens: 100,
        token_budget: 8000,
        selected_chunks: [{ key: 'rules/a', section: 'rules', required: true }],
        omitted_chunks: [],
      },
    })
  );
  process.exit(1);
}

if (mode === 'huge') {
  // Exceed the stdout limit using writeSync; process.stdout.write is asynchronous,
  // so an immediate process.exit() would discard bytes still buffered in the pipe.
  const block = Buffer.alloc(1024 * 1024, 0x61);
  for (let i = 0; i < 12; i++) {
    try {
      fs.writeSync(1, block);
    } catch (e) {
      if (e.code === 'EPIPE') break; // The bridge has closed the pipe after the output limit was exceeded.
      throw e;
    }
  }
  process.exit(0);
}

if (mode === 'slow') {
  setTimeout(() => process.exit(0), 60 * 1000);
  // Keep the process alive.
  setInterval(() => {}, 1000);
} else if (mode === 'multibyte') {
  // Write valid UTF-8 byte by byte; per-chunk decoding would produce U+FFFD.
  const payload = Buffer.from(
    JSON.stringify({ ok: true, title: '任务：源文件索引 — αβγ 🧭', project_revision: 7 }),
    'utf8'
  );
  for (let i = 0; i < payload.length; i++) {
    fs.writeSync(1, payload.subarray(i, i + 1));
  }
  process.exit(0);
} else if (joined.includes('search')) {
  // AWR 0.4.0: `awr search [OPTIONS] [TEXT]` uses positional search text.
  if (argv.includes('--text')) {
    process.stderr.write(
      JSON.stringify({ code: 'InvalidInput', message: "unexpected argument '--text' found" })
    );
    process.exit(2);
  }
  const dashDash = argv.indexOf('--');
  const text = dashDash >= 0 ? argv[dashDash + 1] : null;
  emit({ ok: true, hits: [], query: { text } });
} else if (joined.includes('status')) {
  emit({
    ok: true,
    project_revision: 7,
    current: [],
    current_total: 0,
    ready_count: 1,
    blocked_count: 0,
    organization: { state: 'ready', gaps: [], gap_total: 0, sources: [] },
  });
} else if (joined.includes('ready')) {
  emit({ ok: true, ready: [], ready_total: 0, blocked_sample: [], blocked_total: 0 });
} else if (joined.includes('spec check')) {
  emit({
    version: 1,
    status: 'unknown',
    selection: { spec: null },
    specifications: [],
    references: [],
    requirements: [],
    counts: { covered: 0, uncovered: 0, not_required: 0, needs_review: 0, unknown: 0 },
    diagnostics: ['no_explicit_specifications'],
    association_diagnostics: [],
    read_only: true,
    source_write_performed: false,
    runtime_write_performed: false,
    coverage_basis: 'Explicit registered items and exact task references only; no prose scanning or inferred cancellations.',
    boundary: 'Matched/covered describe declared version bindings and task coverage, never semantic agreement, implementation, acceptance or completion. No source is modified.',
  });
} else if (joined.includes('work assess')) {
  emit({
    version: 1,
    work: argv[argv.length - 1],
    decision: {
      version: 1,
      mode: 'undetermined',
      reasons: [],
      unknown_observations: ['single_outcome', 'bounded_scope'],
      reevaluation_signals: [],
      required_actions: [
        'preserve_identity_intent_scope_and_current_state',
        'assess_unknown_management_facts',
      ],
      optional_maintenance: [],
      completion_policy: 'unchanged_source_policy',
      execution_admission: 'not_granted_by_management_classification',
    },
    observation: null,
    observation_basis: 'host_assertion_not_independently_verified',
    admission_gaps: [],
    record_required: true,
    next_action: 'record_current_assessment_with_explicit_host_observations',
    scope: 'Management requirements only. Readiness, required goals/rules, authorization and completion remain enforced by their existing operations.',
  });
} else if (joined.includes('work show')) {
  emit({ ok: true, project_revision: 7, work: { external_key: argv[argv.length - 1] }, acceptance: [] });
} else if (joined.includes('intake inspect')) {
  emit({ ok: true, project_revision: 7, organization: { sources: [], gaps: [], gap_total: 0 } });
} else if (joined.includes('context compile')) {
  emit({
    ok: true,
    project_revision: 7,
    completeness: { complete: true, project_revision: 7 },
    work_context: {
      rendered_context: 'stub',
      token_estimate: 10,
      token_budget: 5000,
      required_tokens: 10,
      selected_chunks: [],
      omitted_chunks: [],
    },
  });
} else if (joined.includes('source reindex')) {
  emit({ ok: true, project_revision: 8 });
} else if (joined.includes('execution inspect')) {
  emit({
    observation: {
      execution_id: 'exe_4411',
      operation_key: 'live-report',
      purpose: 'slow_child',
      origin_session_id: 11,
      state: 'unknown',
      verified: false,
      basis: 'supervisor_unreachable_without_completion_receipt',
      next_action: 'Inspect the registered execution before retrying',
    },
    side_effects_performed: false,
  });
} else if (joined.includes('recovery inspect')) {
  emit({
    ok: true,
    project_revision: 7,
    session: { id: 11, agent_id: 'stub-agent', status: 'active', work_item_id: 1 },
    checkpoint: {
      id: 3,
      next_action: 'Collect the report then review the failed import',
      open_loops: ['Independent report review remains'],
    },
    executions: [{
      execution_id: 44,
      operation_key: 'live-report',
      purpose: 'slow_child',
      origin_session_id: 11,
      state: 'running',
      verified: true,
      basis: 'loopback supervisor',
      next_action: 'Wait for the supervisor completion event',
    }, {
      execution_id: 45,
      operation_key: 'failed-report',
      purpose: 'failed_child',
      origin_session_id: 11,
      state: 'failed',
      verified: true,
      exit_code: 7,
      basis: 'supervisor completion event',
      next_action: 'Read the result and logs before advancing work',
    }],
    all_execution_states_verified: true,
    source_refresh_performed: false,
    side_effects_performed: false,
    next_step: 'Use doctor for read-only current-source and artifact diagnostics, then session resume.',
  });
} else if (joined.includes('session compaction inspect')) {
  emit({
    ok: true,
    version: 1,
    read_only: true,
    project_revision: 7,
    session: 11,
    state: 'continue',
    observation_event_id: 31,
    post_compaction_percent: 42.0,
    previous_delta_percentage_points: null,
    policy: { post_compaction_threshold_percent: 50 },
    guidance: {
      when: 'Same model/window; completed automatic compaction is below threshold',
      basis: 'Host-reported post-compaction occupancy 42.0%; threshold 50% (heuristic)',
      next_action: 'Continue the current task with native compaction enabled',
      recheck: 'Next completed compaction or changed model/window',
    },
    native_compaction: 'unchanged',
    session_switch_performed: false,
  });
} else if (joined.includes('session show')) {
  emit({
    ok: true,
    project_revision: 7,
    session: {
      id: 11,
      agent_id: 'stub-agent',
      provider: 'stub',
      model: 'stub-model',
      status: 'active',
      work_item_id: 1,
      last_checkpoint_id: 3,
      started_at: 1700000000000,
    },
    checkpoint: {
      id: 3,
      session_id: 11,
      next_action: 'Wire the session show panel',
      open_loops: ['Confirm checkpoint JSON', 'Render open loops'],
      created_at: 1700000000000,
    },
    inherited_checkpoint: null,
    claims: [{
      id: 9,
      work_item_id: 1,
      session_id: 11,
      agent_id: 'stub-agent',
      status: 'active',
      expires_at: 1700003600000,
    }],
    checkpoint_saves: { incomplete_count: 0, attempts: [] },
    resumed_successor: null,
  });
} else if (joined.includes('session list')) {
  emit({
    ok: true,
    project_revision: 7,
    sessions: [{
      id: 11,
      agent_id: 'stub-agent',
      provider: 'stub',
      model: 'stub-model',
      status: 'active',
      work_item_id: 1,
      last_checkpoint_id: 3,
      started_at: 1700000000000,
    }],
    limit: 20,
    may_have_more: false,
  });
} else if (joined.includes('event show')) {
  emit({
    ok: true,
    project_revision: 7,
    source_refresh_performed: false,
    event: {
      id: 21,
      project_id: 1,
      work_item_id: 1,
      session_id: 11,
      branch_id: 1,
      event_type: 'session_started',
      importance: 'normal',
      summary: 'session started',
      payload: { agent_id: 'stub-agent', provider: 'stub' },
      project_revision: 7,
      created_at: 1700000000000,
    },
    payload_included: true,
  });
} else if (joined.includes('event history')) {
  emit({
    ok: true,
    project_revision: 7,
    events: [{
      id: 21,
      type: 'session_started',
      summary: 'session started',
      importance: 'normal',
      session_id: 11,
      work_item_id: 1,
      created_at: 1700000000000,
    }],
    next_cursor: null,
    payloads_included: false,
  });
} else {
  process.stderr.write('unknown stub command\n');
  process.exit(2);
}
