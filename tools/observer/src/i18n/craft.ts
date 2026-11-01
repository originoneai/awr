/** Bilingual field and status vocabulary shared by every observation surface. */
export const FIELD_LABELS: Record<string, readonly [string, string]> = {
  work_id: ['工作项', 'Work item'],
  contract_hash: ['合同摘要', 'Contract digest'],
  candidate_digest: ['候选摘要', 'Candidate digest'],
  candidate_current: ['当前候选', 'Current candidate'],
  selection_version: ['选择版本', 'Selection version'],
  source_revision: ['来源版本', 'Source revision'],
  round_id: ['复核轮次', 'Review round'],
  round_index: ['轮次序号', 'Round index'],
  author_actor_id: ['作者 actor', 'Author actor'],
  author_person_id: ['作者成员', 'Author person'],
  bundle_hash: ['包摘要', 'Bundle digest'],
  artifact_digest: ['制品摘要', 'Artifact digest'],
  decision: ['决定', 'Decision'],
  reviewer_actor_id: ['复核者 actor', 'Reviewer actor'],
  independence_kind: ['独立性类型', 'Independence kind'],
  approval_basis: ['批准依据', 'Approval basis'],
  team_independent_acceptance: ['团队独立验收', 'Team-independent acceptance'],
  reason: ['原因', 'Reason'],
  human_approval: ['人工批准', 'Human approval'],
  criterion: ['验收条件', 'Criterion'],
  rule: ['规则', 'Rule'],
  severity: ['严重性', 'Severity'],
  state: ['状态', 'State'],
  summary: ['摘要', 'Summary'],
  evidence_id: ['证据', 'Evidence'],
  execution_id: ['执行', 'Execution'],
  artifact_id: ['制品', 'Artifact'],
  trust_basis: ['信任依据', 'Trust basis'],
  artifact_verified: ['制品已校验', 'Artifact verified'],
  artifact_verification_basis: ['制品校验依据', 'Artifact verification basis'],
  effects_settled: ['副作用已结清', 'Effects settled'],
  contract_matches_current: ['合同匹配当前', 'Contract matches current'],
  epoch_matches_current: ['纪元匹配当前', 'Epoch matches current'],
  execution_authorized: ['已授权执行', 'Execution authorized'],
  claim_id: ['认领', 'Claim'],
  lease_live: ['租约有效', 'Lease live'],
  current_fence: ['当前栅栏', 'Current fence'],
  expires_at: ['过期时间', 'Expires at'],
  receipt_id: ['回执', 'Receipt'],
  selected_completion_id: ['选中完成回执', 'Selected completion'],
  runtime_state: ['运行状态', 'Runtime state'],
  review_round_id: ['复核轮次', 'Review round'],
  review_decision_id: ['复核决定', 'Review decision'],
  provider: ['提供方', 'Provider'],
  connector_id: ['连接器', 'Connector'],
  connector_version: ['连接器版本', 'Connector version'],
  enabled: ['已启用', 'Enabled'],
  current_epoch: ['当前纪元', 'Current epoch'],
  fact_source: ['事实来源', 'Fact source'],
  resource: ['资源', 'Resource'],
  read_only: ['只读', 'Read-only'],
  integration_id: ['集成', 'Integration'],
  target: ['目标', 'Target'],
  target_resource: ['目标资源', 'Target resource'],
  precondition: ['前置版本', 'Precondition'],
  result_revision: ['结果版本', 'Result revision'],
  outcome: ['结果', 'Outcome'],
  dispatch_state: ['分派状态', 'Dispatch state'],
  current_binding: ['当前绑定', 'Current binding'],
  target_changed: ['目标已变化', 'Target changed'],
  dispatched_at: ['分派时间', 'Dispatched at'],
  source_observation: ['来源观测', 'Source observation'],
  confirmed_fingerprint: ['已确认指纹', 'Confirmed fingerprint'],
  source_fingerprint: ['来源指纹', 'Source fingerprint'],
  source_synchronized: ['来源已同步', 'Source synchronized'],
  cursor_current: ['游标为当前', 'Cursor current'],
  projection_current: ['投影为当前', 'Projection current'],
  pending_publication_id: ['待发布记录', 'Pending publication'],
  metadata_revision: ['元数据版本', 'Metadata revision'],
  history_truncated: ['历史已截断', 'History truncated'],
  complete: ['已完成', 'Complete'],
  'github.submitted': ['GitHub 已提交', 'GitHub submitted'],
  'github.approved': ['GitHub 已批准', 'GitHub approved'],
  'github.merged': ['GitHub 已合并', 'GitHub merged'],
  pr: ['PR', 'PR'],
  webhook_auto_sync: ['Webhook 自动同步', 'Webhook auto-sync'],
  cannot_skip_acceptance_via: ['不可据此跳过验收', 'Cannot skip acceptance via'],
  adoption_available: ['可采用', 'Adoption available'],
  adoption_version_policies: ['采用版本策略', 'Adoption version policies'],
  upstream_source_access: ['上游来源访问', 'Upstream source access'],
  items_count: ['条目数', 'Item count'],
  next_cursor: ['下一游标', 'Next cursor'],
  source_snapshot_id: ['来源快照', 'Source snapshot'],
  project_revision: ['项目版本', 'Project revision'],
  coordinator_epoch: ['协调纪元', 'Coordinator epoch'],
  definition_state: ['定义状态', 'Definition state'],
  authority_version: ['权威版本', 'Authority version'],
  selection_basis: ['选择依据', 'Selection basis'],
  manifest_artifact_id: ['清单制品', 'Manifest artifact'],
  byte_length: ['字节数', 'Byte length'],
  development_source: ['开发记录来源', 'Development source'],
  unknown_means: ['未知含义', 'Unknown means'],
  action: ['操作', 'Action'],
  actor_id: ['认证 actor', 'Actor'],
  client_id: ['客户端', 'Client'],
  result: ['结果', 'Result'],
  id: ['审计行', 'Audit row'],
  credential_id: ['凭据', 'Credential'],
  created_at_unix_ms: ['记录时间', 'Recorded at'],
};

export const STATUS_LABELS: Record<string, readonly [string, string]> = {
  approved: ['已批准', 'Approved'],
  approve: ['批准', 'Approve'],
  rejected: ['已拒绝', 'Rejected'],
  invalidated: ['已失效', 'Invalidated'],
  open: ['进行中', 'Open'],
  confirmed: ['已确认', 'Confirmed'],
  dispatched: ['已分派', 'Dispatched'],
  committed: ['已提交', 'Committed'],
  succeeded: ['成功', 'Succeeded'],
  unknown: ['未知', 'Unknown'],
  claimed: ['已认领', 'Claimed'],
  completed: ['已完成', 'Completed'],
  active: ['有效', 'Active'],
  current: ['当前', 'Current'],
  observed: ['已观测', 'Observed'],
  enabled: ['已启用', 'Enabled'],
  disabled: ['已停用', 'Disabled'],
  passed: ['已通过', 'Passed'],
  failed: ['失败', 'Failed'],
  pending: ['待处理', 'Pending'],
  return: ['已退回', 'Returned'],
  resolved: ['已解决', 'Resolved'],
  blocking: ['阻塞', 'Blocking'],
  applied: ['已应用', 'Applied'],
  consistent: ['一致', 'Consistent'],
  exact: ['精确', 'Exact'],
  current_contract: ['当前合同', 'Current contract'],
  fixed_delivery: ['固定交付', 'Fixed delivery'],
  caller_asserted: ['调用方声明', 'Caller asserted'],
  simulated_member_independent: ['成员独立复核（模拟）', 'Simulated member-independent'],
  simulated_member_independent_review: ['成员独立复核（模拟）', 'Simulated member-independent review'],
  local_git: ['本地 Git', 'Local Git'],
  read_only: ['只读', 'Read-only'],
  green_ci: ['CI 通过', 'Green CI'],
  already_merged: ['已合并', 'Already merged'],
  project_admin: ['项目管理员', 'Project admin'],
  observer: ['观察者', 'Observer'],
  reader: ['只读成员', 'Reader'],
  developer: ['开发者', 'Developer'],
  owned_by_me: ['由我负责', 'Owned by me'],
  owned_by_other: ['由他人负责', 'Owned by other'],
  assigned_to_me: ['指派给我', 'Assigned to me'],
  assigned_to_other: ['指派给他人', 'Assigned to other'],
  handoff_required: ['需要转交', 'Handoff required'],
  responsibility_pending: ['责任待确认', 'Responsibility pending'],
  pool: ['待接手指派', 'Pool'],
};

function zh(language: string): boolean {
  return language.toLowerCase().startsWith('zh');
}

export function fieldLabel(field: string, language: string): string {
  const label = FIELD_LABELS[field];
  return label ? label[zh(language) ? 0 : 1] : field.replaceAll('_', ' ');
}

export function statusLabel(status: string, language: string): string {
  const label = STATUS_LABELS[status];
  return label ? label[zh(language) ? 0 : 1] : status;
}

export function notProvided(language: string): string {
  return zh(language) ? '未提供' : 'Not provided';
}

export function rawObservationValue(value: unknown): string | null {
  if (value == null || value === '') return null;
  if (typeof value === 'string') return value;
  if (typeof value === 'number' || typeof value === 'boolean') return String(value);
  return JSON.stringify(value) ?? null;
}

export function formatMetric(value: number | string, language: string): string {
  const number = typeof value === 'number' ? value : Number(value);
  return Number.isFinite(number) ? new Intl.NumberFormat(language).format(number) : String(value);
}

export function formatObservationValue(value: unknown, language: string): string {
  if (value == null || value === '') return notProvided(language);
  if (typeof value === 'boolean') return value ? (zh(language) ? '是' : 'Yes') : (zh(language) ? '否' : 'No');
  if (typeof value === 'number') return formatMetric(value, language);
  if (Array.isArray(value)) return value.length ? value.map((item) => formatObservationValue(item, language)).join(', ') : notProvided(language);
  if (typeof value === 'string') {
    if (/^-?\d+(?:\.\d+)?$/.test(value)) return formatMetric(value, language);
    return statusLabel(value, language);
  }
  return rawObservationValue(value) ?? notProvided(language);
}
