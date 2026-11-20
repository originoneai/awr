import type { PROJECT_DOCUMENT_STATUSES } from '../core/observation/types';
import type { OverviewInput, OverviewPart, ReadCoverageReason } from '../core/overview/types';
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
  title?: string | null;
  role?: string | null;
}

export interface TeamProjectsResponse {
  ok: boolean;
  projects: TeamProject[];
  session?: { session_id: string; expires_at_ms: number; created_at_ms?: number; last_seen_ms?: number };
  view?: string;
}

/** The shell's own read: which records this deployment's pages read, the organization's
 * display name (null when this deployment does not establish it), plus the perspective
 * facts its directory establishes (null when this deployment does not establish them). */
export interface TeamShellResponse {
  ok: boolean;
  source: 'demo' | 'live';
  organizationName?: string | null;
  perspective: import('../lib/perspective/types').PerspectiveFacts | null;
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

export interface OverviewModelCoverageGap {
  op: string;
  reason: ReadCoverageReason;
  work_id?: string;
  workstream_id?: string;
  error_code?: string;
  next_cursor?: string | null;
}

export interface OverviewModelCoveragePart {
  status: 'complete' | 'partial' | 'unread';
  reason?: ReadCoverageReason;
  missing: OverviewModelCoverageGap[];
  /** No-runtime works have a known empty review history without an upstream read. */
  known_empty?: Array<{
    op: string;
    work_id: string;
    workstream_id: string;
    reason: 'no_runtime';
  }>;
  note?: string;
}

/** One session-scoped composition, directly consumable by deriveOverview. */
export interface TeamOverviewModelResponse {
  ok: true;
  project: string;
  read_at_unix_ms: number;
  input: OverviewInput;
  coverage: Record<OverviewPart, OverviewModelCoveragePart>;
  freshness: TeamFreshness;
  ordering?: 'incomparable';
}

export type TeamIdentity = NonNullable<TeamOverviewResponse['identity']>;

/** AWR-PPD-055: the audit's own identity disclosure. Absent or null fields mean the
 *  directory-read gate did not disclose them; older replies carry no object at all. */
export interface TeamActivityIdentity {
  person_id?: string | null;
  display_name?: string | null;
}

export interface TeamActivityClient {
  display_name?: string | null;
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
  actor?: TeamActivityIdentity | null;
  client?: TeamActivityClient | null;
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

export interface PlanningCandidatesData {
  items: Array<{
    candidate_id: string;
    state: string;
    draft_revision: number;
    author: { display_name: string | null };
    updated_at_unix_ms: number;
    baseline: { matches_current: boolean };
    current_diff: {
      counts: Record<'added' | 'removed' | 'changed' | 'unchanged', number>;
      items: Array<{ work_id: string; title: string; kind: string; fields?: Array<{
        field: string; before: { value: unknown; reason?: string | null }; after: { value: unknown; reason?: string | null };
      }> }>;
    };
    approval: { value: { actor_id: string; display_name: string | null; at_unix_ms: number; self_approved: boolean } | null; reason: string | null };
    publication: { value: { actor_id: string; at_unix_ms: number; source_writeback_pending: boolean } | null; reason: string | null };
    activation: { value: { actor_id: string; at_unix_ms: number; activated_snapshot_id: string; matches_current_snapshot: boolean } | null; reason: string | null };
    rejection?: { value: { actor_id: string; display_name: string | null; at_unix_ms: number; reason: string } | null; reason: string | null };
  }>;
  total: number;
  next_cursor: string | null;
  as_of: { at_unix_ms: number };
}

export interface RequiredSourcesData {
  items: Array<{
    source_position: number;
    path: string | null;
    status: string;
    reason: string | null;
  }>;
  total: number;
  next_cursor: string | null;
  collection: {
    complete: boolean;
    required_total: number;
    checked_at_unix_ms: number;
  };
  as_of: { at_unix_ms: number };
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
  title?: string;
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

export interface OrdinaryDecisionPerson {
  person_id: string;
  display_name: string | null;
}

export interface OrdinaryDecision {
  decision_id: string;
  work_id: string;
  workstream_id: string;
  work_title: string;
  question: string;
  impact: string;
  reply: string | null;
  state: 'open' | 'replied' | 'cancelled';
  baseline_matches_current: boolean;
  requester: OrdinaryDecisionPerson;
  allowed_responders: OrdinaryDecisionPerson[];
  resolution: (OrdinaryDecisionPerson & { at_unix_ms: number }) | null;
  created_at_unix_ms: number;
}

export interface OrdinaryDecisionsData {
  items: OrdinaryDecision[];
  next_cursor: string | null;
  truncated: boolean;
  execution_authorized: false;
  as_of: { at_unix_ms: number };
}

export interface WorkContract {
  goals?: WorkContractValue[];
  acceptance?: WorkContractValue[];
  required_dependencies?: WorkContractValue[];
  [key: string]: unknown;
}

export interface WorkCollaboration {
  delivery_required?: boolean;
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

export interface ReviewRoundsData {
  work_id: string;
  items: Array<{
    round_id: string;
    round_index: number;
    round_position: number;
    readable: boolean;
    state: 'open' | 'approved' | 'rejected' | 'invalidated' | null;
    contract_matches_current: boolean | null;
    reviewer_role: 'independent_reviewer' | 'author_reviewer' | 'agent_reviewer' | 'simulated_member_reviewer' | null;
    reviewer_actor_id: string | null;
    reviewer_person_id: string | null;
    decided_at_unix_ms: number | null;
    reason_first_line: string | null;
  }>;
  total: number;
  next_cursor: string | null;
  as_of: { at_unix_ms: number };
}

/** The data of a source.content reply; text arrives as the service decoded it. */
export interface SourceContentData {
  path?: string;
  sha256?: string;
  byte_length?: number;
  text?: string | null;
  kind?: string;
  snapshot?: string;
  [key: string]: unknown;
}

export interface ProjectDocument {
  content_work_id?: string;
  document_position: number;
  path: string | null;
  name: string | null;
  summary: string | null;
  summary_kind: 'source_excerpt';
  sha256: string | null;
  byte_length: number | null;
  source_kind: 'registered_source';
  readable: boolean;
  status: (typeof PROJECT_DOCUMENT_STATUSES)[number];
  reason: string | null;
}

export interface ProjectDocumentsData {
  work_id: string | null;
  items: ProjectDocument[];
  total: number;
  next_cursor: string | null;
  truncated: boolean;
  read_only: true;
  execution_authorized: false;
  scope: 'work' | 'authorized_workstreams' | 'authorized_works';
  content_read_limit_bytes: number;
  as_of: { at_unix_ms: number; source_snapshot_id: string; project_revision: string };
}

export interface WorkSnapshotData {
  context_complete?: boolean;
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

export interface DependencyVersion {
  receipt_id: string | null;
  contract_hash: string;
  source_snapshot_id: string | null;
}

export interface DependencyObservation {
  work_id: string;
  work_title: string;
  provider_work_id: string;
  provider_title: string | null;
  dependency_position: number;
  version_policy: 'current_contract' | 'fixed_delivery';
  review_assurance: string | { cross_workstream: { review_assurance: string; version_policy: string } } | null;
  required_version: DependencyVersion | null;
  available_version: DependencyVersion | null;
  adopted_version: DependencyVersion | null;
  consumed_version: DependencyVersion | null;
  adoption_valid: boolean | null;
  validity: 'valid' | 'upstream_changed' | 'unavailable';
  reason: string | null;
  missing: Record<string, string | null>;
}

export interface DependencyListData {
  work_id: string | null;
  items: DependencyObservation[];
  total: number;
  next_cursor: string | null;
  as_of: { at_unix_ms: number };
}

export interface HandoffListData {
  items: Array<{
    handoff_id: string;
    kind: 'execution' | 'responsibility';
    effective_status: string;
    pending: boolean;
    expired: boolean;
    from_display_name: string | null;
    to_display_name: string | null;
    updated_at_unix_ms: number;
    expires_at_unix_ms: number | null;
  }>;
  total: number;
  next_cursor: string | null;
  as_of: { at_unix_ms: number };
}
