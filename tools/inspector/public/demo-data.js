/**
 * Demo data
 *
 * Use this data when the bridge is unavailable or the project is not initialized.
 *
 * Match the actual JSON shapes in the repository:
 *   status -> crates/awr-runtime/src/status_action.rs (four queues plus omissions)
 *   item   -> brief() in crates/awr-runtime/src/status_summary.rs
 *   work   -> crates/awr-cli/src/query.rs (nested work and string-array acceptance)
 *   context → crates/awr-context/src/{compile,budget}.rs（work_context.selected_chunks）
 *
 * Demo mode exercises the same field mappings as live mode.
 * Work items and gaps are synthetic; token counts come from the public benchmark.
 */

(function () {
  'use strict';

  const i18n = typeof module !== 'undefined' && module.exports
    ? require('./i18n.js') : window.AWR_I18N;

  const details = (key) => ({ cli: ['work', 'show', key], mcp: 'awr_work_get' });

  const item = (o) => Object.assign({
    key: o.key,
    title: o.title,
    status: o.status,
    raw_status: o.raw_status || o.status,
    owner: o.owner || null,
    next_action: o.next_action || null,
    blocker: o.blocker || null,
    revision: 128,
    source_revision: o.source_revision || 41,
    details: details(o.key),
  }, o.extra || {});

  const CURRENT = [
    item({
      key: 'EXAMPLE-001', title: i18n.t('ui.deterministic_context_compilation'), status: 'in_progress', owner: 'lin',
      next_action: i18n.t('ui.finish_the_third_renderer_pass_then_follow'),
      extra: {
        ownership_required: true,
        codes: [],
        claims: [{ id: 'clm_01', session_id: 'sess_8f21', agent_id: 'coding-agent', expires_at: iso(-45) }],
      },
    }),
  ];

  const READY = [
    item({
      key: 'EXAMPLE-005', title: i18n.t('ui.windows_path_normalization'), status: 'planned',
      next_action: i18n.t('ui.reproduce_the_flaky_test_on_windows_latest'),
      extra: { codes: [] },
    }),
    item({
      key: 'EXAMPLE-006', title: i18n.t('ui.checkpoint_compaction_strategy'), status: 'planned',
      next_action: i18n.t('ui.design_agreed_waiting_for_example_002'),
      extra: { codes: [] },
    }),
  ];

  const WAITING = [
    item({
      key: 'EXAMPLE-004', title: i18n.t('ui.run_intake_inspect_on_an_existing_repository'), status: 'in_review', owner: 'mei',
      next_action: i18n.t('demo.collect_reply'),
      extra: { wait_ids: [9021], wait_total: 1, execution_total: 0, executions: [], codes: [] },
    }),
    item({
      key: 'EXAMPLE-002', title: i18n.t('ui.rebuild_the_sqlite_projection_after_source_drift'), status: 'planned',
      next_action: i18n.t('demo.continue_prerequisite'),
      extra: { wait_total: 0, codes: ['dependency_not_completed'] },
    }),
  ];

  const BLOCKED = [
    item({
      key: 'EXAMPLE-003', title: i18n.t('ui.mcp_tool_surface_for_context_compile'), status: 'blocked',
      blocker: i18n.t('ui.ol_09_handshake_returns_a_stale_project'),
      next_action: i18n.t('ui.investigate_why_the_symlink_path_handshake_returns'),
      extra: { codes: ['source_blocked'], structural_codes: [] },
    }),
    item({
      key: 'EXAMPLE-008', title: i18n.t('ui.remove_the_yaml_front_matter_fallback'), status: 'blocked',
      blocker: i18n.t('ui.ol_19_the_decision_is_still_pending'),
      next_action: i18n.t('ui.write_the_decision_record_first'),
      extra: { codes: ['missing_decision_record'], structural_codes: ['goal_not_linked'] },
    }),
  ];

  // Full work show responses, indexed by external work key.
  const DETAIL = {
    'EXAMPLE-001': {
      acceptance: [
        i18n.t('demo.packet_budget'),
        i18n.t('demo.facts_round_trip'),
        i18n.t('demo.compile_latency'),
      ],
      required_dependencies: [],
      missing_dependencies: [],
      milestone: 'goal#demo',
      spec_refs: [{
        specification: 'configuration-spec',
        item: 'NEW',
        source_fingerprint: 'sha256:' + 'a'.repeat(64),
        criterion: i18n.t('demo.packet_budget'),
      }],
    },
    'EXAMPLE-002': {
      acceptance: [
        i18n.t('demo.detect_drift'),
        i18n.t('demo.rebuild_changed'),
      ],
      required_dependencies: [{ external_key: 'EXAMPLE-001', status: 'in_progress', revision: 128 }],
      missing_dependencies: [],
      milestone: 'goal#demo',
    },
    'EXAMPLE-003': {
      acceptance: [
        i18n.t('demo.tool_schemas'),
        i18n.t('demo.absolute_root'),
      ],
      required_dependencies: [{ external_key: 'EXAMPLE-001', status: 'in_progress', revision: 128 }],
      missing_dependencies: [],
      milestone: 'goal#demo',
    },
    'EXAMPLE-004': {
      acceptance: [
        i18n.t('demo.dry_run'),
        i18n.t('demo.matched_files'),
        i18n.t('demo.exit_code'),
      ],
      required_dependencies: [],
      missing_dependencies: [],
      milestone: 'goal#demo',
    },
    'EXAMPLE-005': {
      acceptance: [
        i18n.t('demo.windows_paths'),
        i18n.t('demo.windows_ci'),
      ],
      required_dependencies: [],
      missing_dependencies: [],
      milestone: 'goal#demo',
    },
    'EXAMPLE-006': {
      acceptance: [
        i18n.t('demo.preserve_open_loops'),
        i18n.t('demo.history_replay'),
      ],
      required_dependencies: [{ external_key: 'EXAMPLE-002', status: 'planned', revision: 128 }],
      missing_dependencies: [],
      milestone: 'goal#demo',
    },
    'EXAMPLE-008': {
      acceptance: [
        i18n.t('demo.decision_indexed'),
        i18n.t('demo.migration_note'),
      ],
      required_dependencies: [{ external_key: 'EXAMPLE-004', status: 'in_review', revision: 128 }],
      missing_dependencies: ['EXAMPLE-011'],
      milestone: 'goal#demo',
    },
  };

  // Only some items have evidence/decisions, demonstrating populated and empty states.
  const DETAIL_EXTRA = {
    'EXAMPLE-001': {
      evidence: [
        { external_key: 'bench-p95-0917', evidence_type: 'benchmark', level: 'locally_verified' },
      ],
      decisions: [{ external_key: 'dec-0003', title: i18n.t('ui.split_renderer_chunks_by_paragraph_rather_than') }],
    },
    'EXAMPLE-004': {
      evidence: [
        { external_key: 'dry-run-0915', evidence_type: 'command', level: 'locally_verified' },
        { external_key: 'inspect-report-02', evidence_type: 'report', level: 'locally_verified' },
      ],
      decisions: [],
    },
  };

  const ALL = CURRENT.concat(READY, WAITING, BLOCKED);

  window.AWR_DEMO = {
    health: { mode: 'demo', project: '.local/demo', awrVersion: null, bridgeVersion: '1.0.0' },

    status: {
      view: 'action',
      schema_version: 1,
      project: 'demo',
      project_id: 1,
      project_revision: 128,
      total: 7,
      project_work_total: 8,
      current_total: 1,
      ready_count: 4,
      waiting_count: 2,
      blocked_count: 2,
      current: CURRENT,
      ready: READY,
      waiting: WAITING,
      blocked: BLOCKED,
      omissions: { current: 0, ready: 2, waiting: 0, blocked: 0 },
      freshness_basis: 'source_refresh',
      guidance: {
        when: i18n.t('demo.continue_work'),
        next_action: 'Prepare the selected work; continue only with its owned session or explicitly resume it',
      },
      pending_operations: {
        basis: 'project-wide registered runtime findings',
        total: 2,
        items: [
          { code: 'active_session', kind: 'session', id: 101 },
          { code: 'execution_outcome_unknown', kind: 'execution', id: 'exe_4411' },
        ],
      },
      organization: {
        state: 'partially_structured',
        business_execution_ready: false,
        gap_total: 3,
        project_gap_total: 3,
        gaps: [
          { code: 'goal_not_linked', target: 'EXAMPLE-008', detail: i18n.t('ui.this_work_item_has_no_linked_goal') },
          { code: 'acceptance_missing', target: 'EXAMPLE-011', detail: i18n.t('ui.the_source_does_not_declare_acceptance_criteria') },
          { code: 'plan_not_found', target: 'project', detail: i18n.t('ui.the_project_has_no_indexable_plan_source') },
        ],
      },
    },

    /** The work show response. */
    workShow(key) {
      const brief = ALL.find((w) => w.key === key) || ALL[0];
      const extra = DETAIL[brief.key] || { acceptance: [], required_dependencies: [], missing_dependencies: [] };
      return Object.assign({
        ok: true,
        project_revision: 128,
        freshness_basis: 'source_refresh',
        read_only: false,
        work: Object.assign({}, brief, {
          ready: brief.status === 'planned',
          diagnostics: brief.codes || [],
          active_claims: brief.claims || [],
        }),
        evidence: DETAIL_EXTRA[brief.key] ? DETAIL_EXTRA[brief.key].evidence : [],
        decisions: DETAIL_EXTRA[brief.key] ? DETAIL_EXTRA[brief.key].decisions : [],
        dependency_cycles: [],
      }, extra);
    },

    sources: {
      project_revision: 128,
      summary: { markdown: 7, yaml: 2, manifest: 1 },
      files: [
        { path: 'docs/architecture.md', kind: 'markdown', items: 12, state: 'indexed', indexed_at: iso(6) },
        { path: 'docs/work/EXAMPLE-001.md', kind: 'markdown', items: 9, state: 'drift', indexed_at: iso(4320) },
        { path: 'docs/work/EXAMPLE-002.md', kind: 'markdown', items: 6, state: 'indexed', indexed_at: iso(6) },
        { path: 'docs/work/EXAMPLE-004.md', kind: 'markdown', items: 7, state: 'indexed', indexed_at: iso(6) },
        { path: 'docs/work/EXAMPLE-008.md', kind: 'markdown', items: 3, state: 'new', indexed_at: null },
        { path: 'docs/bench/README.md', kind: 'markdown', items: 5, state: 'indexed', indexed_at: iso(4320) },
        {
          path: 'docs/scratch-config.md', kind: 'markdown', items: 0, state: 'rejected', indexed_at: null,
          rejection: {
            rule: 'secret-boundaries/assigned-credential',
            location: { line: 42, column: 3 },
            repair: i18n.t('ui.move_assigned_credentials_to_environment_variables_or'),
          },
        },
        { path: 'agents/handoff.yaml', kind: 'yaml', items: 4, state: 'indexed', indexed_at: iso(1440) },
        { path: 'agents/roles.yaml', kind: 'yaml', items: 2, state: 'indexed', indexed_at: iso(1440) },
        { path: 'project.toml', kind: 'manifest', items: null, state: 'indexed', indexed_at: iso(6) },
      ],
    },

    sessions: [
      {
        id: 101,
        agent_id: 'lin',
        provider: 'cursor',
        model: 'composer',
        status: 'active',
        work_item_id: 1,
        last_checkpoint_id: 7,
        started_at: Date.now() - 12 * 60000,
      },
    ],

    sessionShows: {
      101: {
        session: {
          id: 101,
          agent_id: 'lin',
          provider: 'cursor',
          model: 'composer',
          status: 'active',
          work_item_id: 1,
          last_checkpoint_id: 7,
        },
        checkpoint: {
          id: 7,
          session_id: 101,
          next_action: i18n.t('demo.session_show_next'),
          open_loops: [i18n.t('demo.session_show_loop')],
          created_at: Date.now() - 4 * 60000,
        },
        inherited_checkpoint: null,
        claims: [{
          id: 8,
          work_item_id: 1,
          session_id: 101,
          agent_id: 'lin',
          status: 'active',
          expires_at: Date.now() + 50 * 60000,
        }],
        checkpoint_saves: { incomplete_count: 0, attempts: [] },
        resumed_successor: null,
      },
    },

    compactionInspects: {
      101: {
        ok: true,
        version: 1,
        read_only: true,
        session: 101,
        state: 'continue',
        observation_event_id: 31,
        post_compaction_percent: 42.0,
        previous_delta_percentage_points: null,
        policy: { post_compaction_threshold_percent: 50 },
        guidance: {
          when: i18n.t('demo.compaction_when'),
          basis: i18n.t('demo.compaction_basis'),
          next_action: i18n.t('demo.compaction_next'),
          recheck: i18n.t('demo.compaction_recheck'),
        },
        native_compaction: 'unchanged',
        session_switch_performed: false,
      },
    },

    recoveryInspects: {
      101: {
        session: { id: 101, agent_id: 'lin', status: 'active', work_item_id: 1 },
        checkpoint: {
          id: 7,
          next_action: i18n.t('demo.session_show_next'),
          open_loops: [i18n.t('demo.session_show_loop')],
        },
        executions: [{
          execution_id: 'exe_4411',
          operation_key: 'live-report',
          purpose: 'slow_child',
          origin_session_id: 101,
          state: 'unknown',
          verified: false,
          basis: 'missing supervisor',
          next_action: 'Inspect the registered execution before retrying',
        }],
        all_execution_states_verified: false,
        source_refresh_performed: false,
        side_effects_performed: false,
        next_step: 'Use doctor, then session resume. Do not retry automatically.',
      },
    },

    workAssess(key) {
      const gaps = key === 'EXAMPLE-008' ? ['goal_context_missing'] : [];
      return {
        version: 1,
        work: key,
        decision: {
          version: 1,
          mode: 'undetermined',
          reasons: [],
          unknown_observations: [
            'single_outcome',
            'bounded_scope',
            'single_executor',
            'no_deferred_wait',
            'plan_valid',
            'outcome_known',
            'independently_schedulable_units',
          ],
          reevaluation_signals: [],
          required_actions: [
            'preserve_identity_intent_scope_and_current_state',
            'consume_required_context_and_hard_rules',
            'retain_completion_basis_and_actual_outcome',
            'check_source_versions_permissions_claims_and_request_identity',
            'assess_unknown_management_facts',
          ],
          optional_maintenance: [],
          completion_policy: 'unchanged_source_policy',
          execution_admission: 'not_granted_by_management_classification',
        },
        observation: null,
        observation_basis: 'host_assertion_not_independently_verified',
        admission_gaps: gaps,
        record_required: true,
        next_action: gaps.length
          ? 'repair_required_context_before_execution'
          : 'record_current_assessment_with_explicit_host_observations',
        scope: 'Management requirements only. Readiness, required goals/rules, authorization and completion remain enforced by their existing operations.',
      };
    },

    specCheck() {
      return {
        version: 1,
        status: 'needs_review',
        selection: { spec: null },
        specifications: [{
          key: 'configuration-spec',
          version: '2',
          status: 'current',
          item_count: 2,
        }],
        references: [{
          work: 'EXAMPLE-001',
          reference: {
            specification: 'configuration-spec',
            item: 'NEW',
            source_fingerprint: 'sha256:' + 'a'.repeat(64),
            criterion: i18n.t('demo.packet_budget'),
          },
          status: 'current',
          reasons: [],
        }],
        requirements: [
          {
            specification: 'configuration-spec',
            item: 'NEW',
            status: 'covered',
            work_items: ['EXAMPLE-001'],
          },
          {
            specification: 'configuration-spec',
            item: 'EXPORT',
            status: 'uncovered',
            work_items: [],
          },
        ],
        counts: { covered: 1, uncovered: 1, not_required: 0, needs_review: 0, unknown: 0 },
        diagnostics: [],
        association_diagnostics: [],
        read_only: true,
        source_write_performed: false,
        runtime_write_performed: false,
        coverage_basis: 'Explicit registered items and exact task references only; no prose scanning or inferred cancellations.',
        boundary: 'Matched/covered describe declared version bindings and task coverage, never semantic agreement, implementation, acceptance or completion. No source is modified.',
      };
    },

    executionInspects: {
      exe_4411: {
        observation: {
          execution_id: 'exe_4411',
          operation_key: 'live-report',
          purpose: 'slow_child',
          origin_session_id: 101,
          state: 'unknown',
          verified: false,
          basis: 'missing supervisor',
          next_action: 'Inspect the registered execution before retrying',
        },
        side_effects_performed: false,
      },
    },

    events: [
      {
        id: 501,
        type: 'checkpoint_saved',
        summary: i18n.t('demo.checkpoint_saved'),
        importance: 'normal',
        session_id: 101,
        work_item_id: 1,
        created_at: Date.now() - 4 * 60000,
      },
      {
        id: 500,
        type: 'session_started',
        summary: i18n.t('demo.session_started'),
        importance: 'normal',
        session_id: 101,
        work_item_id: 1,
        created_at: Date.now() - 12 * 60000,
      },
    ],

    eventShows: {
      501: {
        source_refresh_performed: false,
        event: {
          id: 501,
          work_item_id: 1,
          session_id: 101,
          event_type: 'checkpoint_saved',
          importance: 'normal',
          summary: i18n.t('demo.checkpoint_saved'),
          payload: { checkpoint_id: '7', digest: 'Record the work actually done' },
          created_at: Date.now() - 4 * 60000,
        },
        payload_included: true,
      },
      500: {
        source_refresh_performed: false,
        event: {
          id: 500,
          work_item_id: 1,
          session_id: 101,
          event_type: 'session_started',
          importance: 'normal',
          summary: i18n.t('demo.session_started'),
          payload: { agent_id: 'lin', provider: 'cursor' },
          created_at: Date.now() - 12 * 60000,
        },
        payload_included: true,
      },
    },

    /** The context compile response; budgets below 5,000 omit optional chunks. */
    compile(key, budget) {
      const brief = ALL.find((w) => w.key === key) || ALL[0];
      const extra = DETAIL[brief.key] || { acceptance: [], required_dependencies: [] };
      const tight = budget < 5000;

      const chunks = [
        { key: 'rules/project', section: 'rules', required: true },
        { key: 'rules/adapters', section: 'rules', required: true },
        { key: 'goal/demo', section: 'goal', required: true },
        { key: 'work/' + brief.key, section: 'work', required: true },
        { key: 'acceptance/' + brief.key, section: 'acceptance', required: true },
        { key: 'dependencies/' + brief.key, section: 'dependencies', required: true },
        { key: 'loops/open', section: 'loops', required: false },
        { key: 'source/architecture#1', section: 'source', required: false },
        { key: 'source/architecture#2', section: 'source', required: false },
        { key: 'source/architecture#3', section: 'source', required: false },
      ];
      const selected = tight ? chunks.filter((c) => c.required || c.section === 'loops') : chunks;
      const omitted = chunks
        .filter((c) => selected.indexOf(c) < 0)
        .map((c) => ({ key: c.key, section: c.section, reason: 'insufficient_budget_for_whole_chunk' }));

      const lines = [
        '# work: ' + brief.key,
        '# goal: ' + (extra.milestone || 'goal#demo'),
        '# revision: 128',
        '',
        i18n.t('ui.required_rules'),
        i18n.t('ui.markdown_yaml_are_authoritative_sqlite_is_a'),
        i18n.t('ui.every_mutation_must_include_the_revision_you'),
        '',
        i18n.t('ui.acceptance_criteria_verbatim_from_source'),
      ].concat(
        extra.acceptance.map((a) => '- ' + a),
        [
          '',
          i18n.t('ui.dependencies_187'),
          extra.required_dependencies.length
            ? extra.required_dependencies.map((d) => `- ${d.external_key}（${d.status}）`).join('\n')
            : i18n.t('ui.none'),
          '',
          i18n.t('ui.blocker_189'),
          brief.blocker ? '- ' + brief.blocker : i18n.t('ui.none'),
          '',
          i18n.t('ui.next_step_190'),
          '- ' + (brief.next_action || i18n.t('ui.not_specified_in_the_source')),
          '',
          omitted.length
            ? i18n.t('ui.p0_optional_chunks_omitted_due_to_budget', { p0: omitted.length })
            : i18n.t('ui.all_selected_chunks_are_included'),
        ]
      );

      return {
        ok: omitted.length === 0,
        level: 'L1',
        project_revision: 128,
        completeness: {
          complete: omitted.length === 0,
          status: omitted.length === 0 ? 'CONTEXT COMPLETE' : 'CONTEXT INCOMPLETE',
          project_revision: 128,
          rules_complete: true,
          goal_context_complete: true,
          work_state_complete: true,
          acceptance_complete: true,
          dependencies_complete: (extra.required_dependencies || []).length === 0,
          source_fresh: !tight,
          issues: [],
          evidence_gaps: (DETAIL_EXTRA[brief.key] && DETAIL_EXTRA[brief.key].evidence.length)
            ? []
            : [{ code: 'no_evidence', reason: 'no evidence is associated with this work', reference: brief.key }],
          unresolved_required_dependencies: (extra.required_dependencies || [])
            .filter((d) => d.status !== 'done')
            .map((d) => ({ external_key: d.external_key })),
        },
        omitted_refs: [],
        work_context: {
          rendered_context: lines.join('\n'),
          context_hash: '9f3c1ad2e7b04c58',
          token_estimate: tight ? 3311 : 4998,
          required_tokens: 3311,
          token_budget: budget,
          tokenizer: 'o200k_base',
          selected_chunks: selected,
          omitted_chunks: omitted,
        },
      };
    },
  };

  function iso(minutesAgo) {
    return new Date(Date.now() - minutesAgo * 60000).toISOString();
  }
})();
