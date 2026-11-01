import type { PlaneState } from './contract';

/** One freshness plane as the bridge reports it (see observation-contract). */
export interface PlaneFreshness {
  status: PlaneState;
  at_unix_ms?: number;
  error_code?: string;
  stale_after_ms?: number;
  age_ms?: number;
  [key: string]: unknown;
}

export interface TeamFreshness {
  report_received: PlaneFreshness;
  authoritative: PlaneFreshness;
  page_read: PlaneFreshness;
}

export interface TeamProject {
  key: string;
  title: string;
  role: string;
}

export interface TeamProjectsResponse {
  ok: boolean;
  projects: TeamProject[];
  session?: { session_id: string; expires_at_ms: number; created_at_ms?: number; last_seen_ms?: number };
  view?: string;
}

export interface CoveragePart {
  workstream_id?: string;
  status: 'succeeded' | 'failed';
  error_code?: string;
  enumerated?: number;
  works_enumerated?: number;
  works_total?: number | null;
  truncated?: boolean;
  next_cursor?: unknown;
  read_at_unix_ms?: number;
}

export interface TeamWork {
  key: string;
  title: string;
  status: string | null;
  owner_person: unknown;
  agent: unknown;
  next_step: string | null;
  detail_loaded: boolean;
  workstream_id: string;
  contract_hash: string;
}

export interface TeamOverviewResponse {
  ok: boolean;
  project: string;
  works: TeamWork[];
  workstreams: Array<{ id: string; external_key?: string; title?: string }>;
  members?: Array<Record<string, unknown>>;
  coverage: {
    scope: CoveragePart;
    streams: CoveragePart[];
    works_enumerated: number;
    works_total: number | null;
    succeeded: number;
    failed: number;
    truncated: boolean;
  };
  freshness: TeamFreshness;
  schema?: string;
  error?: { code: string; message: string };
  identity?: {
    actor_id: string;
    client_id?: string;
    role: string;
    business_roles?: string[];
    can_manage_members?: boolean;
    can_read_project_audit?: boolean;
    membership_action_ceiling?: string[];
  };
  mcp_url?: string;
  interaction_mode?: string;
}

export interface TeamActivityItem {
  action: string;
  actor_id: string;
  client_id: string | null;
  created_at_unix_ms: number;
  credential_id: string | null;
  id: string;
  result: unknown;
  work_id: string | null;
}

export interface TeamActivityResponse {
  ok: true;
  data: {
    items: TeamActivityItem[];
    next_cursor: unknown;
    development_source: unknown;
    chat_text_collected: false;
    tool_io_collected: false;
    request_retention: unknown;
    request_retention_capacity?: number;
    scope: unknown;
    unknown_means: unknown;
  };
}

export interface TeamReadResponse<T> {
  ok: true;
  result: {
    data: T;
    authority_version?: string;
    selection_basis?: string;
    project_revision?: string;
    protocol_version?: number;
    scope_id?: string;
    source_snapshot_id?: string;
    coordinator_epoch?: string;
  };
  query?: Record<string, unknown>;
  freshness: TeamFreshness;
}

export interface WorkQuery {
  op: string;
  protocol_version: 1;
  work_id: string;
  workstream_id: string;
}

export type WorkNavigation =
  | 'resume'
  | 'recovery_required'
  | 'handoff_required'
  | 'held'
  | 'waiting_external'
  | 'waiting_dependency'
  | 'observe'
  | 'prepare';

export interface WorkNextItem {
  navigation: WorkNavigation;
  work_id: string;
  workstream_id: string;
  state: 'claimed' | null;
  title: string;
  responsibility: {
    relation: 'owned_by_me' | 'pool';
    owner: string | null;
    version: string;
  };
  next_query: WorkQuery;
}

export interface WorkResumeItem {
  work_id: string;
  session_id: string;
  session_version: string;
  next_query: WorkQuery;
}

export interface WorkNextData {
  identity: { actor_id: string; role: string; business_roles: string[] };
  items: WorkNextItem[];
  resume: WorkResumeItem[];
  next_cursor: unknown;
  next_action?: unknown;
  recheck_on?: unknown[];
  execution_authorized: false;
}

export interface WorkInboxItem {
  item_key: string;
  title: string;
  title_truncated: boolean;
  work_id: string;
  workstream_id: string;
  contract_hash: string;
  guidance?: {
    code: string;
    action?: { op: string; note?: string; query?: Record<string, unknown> };
    because?: unknown[];
  };
  next_query: WorkQuery;
}

export interface WorkInboxData {
  items: WorkInboxItem[];
  next_cursor: unknown;
  execution_authorized: false;
}

export type WorkContractValue = string | number | boolean | Record<string, unknown> | null;

export interface WorkContract {
  goals?: WorkContractValue[];
  acceptance?: WorkContractValue[];
  required_dependencies?: WorkContractValue[];
  [key: string]: unknown;
}

export interface WorkCollaboration {
  candidate?: {
    current?: boolean;
    digest?: string;
    selection_version?: string;
    required_checks?: string[];
  };
  review?: {
    id?: string;
    state?: string;
    current_candidate?: boolean;
    bundle_hash?: string;
    author_actor_id?: string;
  } | null;
  integration?: {
    id?: string;
    state?: string;
    current_binding?: boolean;
    target_changed?: boolean;
    reviewed_execution_id?: string;
  } | null;
  publication?: {
    id?: string;
    phase?: string;
    metadata_revision?: string;
    basis?: string;
  } | null;
  source_state?: {
    observation?: string;
    source_synchronized?: boolean;
    query_started_at_unix_ms?: number;
    [key: string]: unknown;
  } | null;
  verification?: Array<{ check?: string; run_id?: string; outcome?: string }>;
  facts_truncated?: boolean;
}

export interface WorkSnapshotData {
  work_id?: string;
  contract_hash?: string;
  definition_state?: string;
  runtime?: { state?: string; work_version?: string };
  observation?: {
    last_activity_at_unix_ms?: number | null;
    observed_at_unix_ms?: number | null;
    progress?: Record<string, unknown> | null;
    reporting?: { client_reported_at_unix_ms?: number | null } & Record<string, unknown>;
    client?: { actor_id?: string; actor_name?: string; client_id?: string; capabilities?: Record<string, unknown> };
    runtime?: { state?: string; work_version?: string; selected_completion_id?: string | null };
    responsibility?: Record<string, unknown>;
    session?: Record<string, unknown> | null;
    claim?: Record<string, unknown> | null;
    execution?: { claim_id?: string; execution_id?: string };
    collaboration?: WorkCollaboration | null;
  };
  visible_contract?: WorkContract | null;
  published_contract?: WorkContract | null;
  required_specs?: Array<{
    path: string;
    sha256: string;
    byte_length?: number;
    content_included?: boolean;
    text?: string;
    next_step?: unknown;
  }>;
  authorized_readable_refs?: Array<{
    kind: string;
    path: string;
    sha256: string;
    byte_length?: number;
  }>;
  snapshot?: Record<string, unknown>;
  guidance?: {
    code?: string;
    action?: { op?: string; note?: string };
  } | null;
  next_step?: string | null;
}

export interface ReviewDecision {
  decision: string;
  decision_id: string;
  approval_basis: string;
  human_approval: boolean;
  independence_kind: string;
}

export interface ReviewData {
  review: {
    decisions: ReviewDecision[];
    author_actor_id: string;
    author_person_id: string;
    artifact_digest: string;
    contract_hash: string;
    bundle_hash: string;
  } | null;
}
