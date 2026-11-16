import snapshot from 'awr-progress-map/project-map/snapshot.js';
import { readsAsName, workLabel } from '../core/observation';

export interface GraphWork {
  work_id: string;
  external_key: string;
  workstream_id: string;
  title: string;
  state: string;
  definition_state?: string;
  owner: string | null;
  goals?: string[];
  goal_titles?: Array<{ id: string; title: string }>;
  historical_responsibility?: { name: string; provenance: 'historical' } | null;
  last_event_at: number | null;
  dependencies_complete: boolean;
  required_dependencies: string[];
  claims: Array<{
    id: string; actor_id: string; actor_name: string | null; session_id: string;
    acquired_at_unix_ms: number; expires_at_unix_ms: number;
  }>;
}

export interface TeamGraphResponse {
  ok: true;
  project: string;
  demo?: boolean;
  items: GraphWork[];
  workstreams: Array<{ id: string; title: string }>;
  as_of: { project_revision: string; source_snapshot_id: string; coordinator_epoch: string } | null;
  read_at_unix_ms: number;
}

/** Adapt current Team facts to the same snapshot consumed by the personal views. */
export function teamProjectSnapshot(graph: TeamGraphResponse, language = 'en-US', projectName = graph.project) {
  const keys = new Map(graph.items.map((item) => [item.work_id, item.external_key]));
  if (keys.size !== graph.items.length || new Set(keys.values()).size !== keys.size) {
    throw new Error('Duplicate graph work identity');
  }
  const compare = (a: string, b: string) => a < b ? -1 : a > b ? 1 : 0;
  const actors = new Map([...new Set(graph.items.flatMap(item => item.claims.map(claim => claim.actor_id)))].sort(compare).map((id, index) => [id, `Agent ${index + 1}`]));
  const members = new Map([...new Set(graph.items.flatMap(item => item.owner ? [item.owner] : []))].sort(compare).map((id, index) => [id, `Member ${index + 1}`]));
  const actorName = (claim: GraphWork['claims'][number]) => claim.actor_name && readsAsName(claim.actor_name) ? claim.actor_name : actors.get(claim.actor_id);
  const groupFor = (item: GraphWork) => {
    const goal = (item.goals ?? []).map(id => item.goal_titles?.find(goal => goal.id === id)).find(Boolean);
    const stream = graph.workstreams.find(stream => stream.id === item.workstream_id);
    const group = goal ? { id: `goal:${goal.id}`, name: goal.title }
      : stream ? { id: `stream:${stream.id}`, name: stream.title } : null;
    const name = workLabel(group?.name, { language, recordedTitle: true });
    return name.kind === 'named' ? { ...group!, name: name.text }
      : { id: 'project', name: workLabel(projectName, { language, recordedTitle: true }).text };
  };
  const nodes = graph.items.map((item) => ({
    id: item.work_id, key: item.external_key, title: workLabel(item.title, { language, recordedTitle: true }).text,
    display_key: '',
    status: item.state === 'unclaimed' ? 'planned' : snapshot.STATUSES.includes(item.state) ? item.state : 'unknown',
    source_status: item.state,
    owner: item.owner && readsAsName(item.owner) ? item.owner : item.owner ? members.get(item.owner)
      : item.historical_responsibility?.provenance === 'historical' && readsAsName(item.historical_responsibility.name)
        ? `${item.historical_responsibility.name} ${language.startsWith('zh') ? '（历史记录）' : '(historical record)'}` : null,
    goals: item.goals ?? [],
    group: groupFor(item),
    last_event_at: item.last_event_at, blocker: null, milestone: null,
    ready: null, archived: item.definition_state === 'archived', next_action: null, waits: [], diagnostics: [],
    claims: item.claims.map((claim) => ({
      id: claim.id, agent_id: actorName(claim), session_id: claim.session_id,
      acquired_at: claim.acquired_at_unix_ms, expires_at: claim.expires_at_unix_ms,
    })),
  })).sort((a, b) => compare(a.key, b.key));
  const edges = graph.items.flatMap((item) => item.required_dependencies.map((dependency) => {
    const prerequisite = keys.get(dependency);
    if (!prerequisite) throw new Error('Graph dependency enumeration is incomplete');
    return { dependent: item.external_key, prerequisite, required: true };
  })).sort((a, b) => compare(a.dependent, b.dependent) || compare(a.prerequisite, b.prerequisite));
  const sessions = graph.items.flatMap((item) => item.claims.map((claim) => ({
    id: claim.session_id, agent_id: actorName(claim),
    work_key: item.external_key, status: 'active', started_at: null,
    last_event_at: null, last_checkpoint_id: null,
  })));
  return snapshot.seal({
    schema: snapshot.SCHEMA, version: snapshot.VERSION,
    project: { key: graph.project, name: workLabel(projectName, { language, recordedTitle: true }).text, id: graph.project, revision: graph.as_of?.project_revision ?? 'demo' },
    basis: {
      work_graph: {
        graph_fingerprint: `sha256:${snapshot.sha256Hex(snapshot.canonical({ nodes, edges }))}`,
        node_count: nodes.length, edge_count: edges.length,
      },
      team: graph.as_of, sessions: { count: sessions.length, truncated: false },
    },
    nodes, edges, sessions, goals: [...new Map(graph.items.flatMap(item => (item.goal_titles ?? []).map(goal =>
      [goal.id, { ...goal, title: workLabel(goal.title, { language, recordedTitle: true }).text }] as const))).values()],
    doctor: { ok: null, findings: [] },
    unavailable: [
      { field: 'node.milestone', reason: 'Team graph has no milestone membership; swimlanes use the recorded work groups.' },
      { field: 'node.blocker', reason: 'Detailed reports are available through the authorized task read.' },
      { field: 'node.ready', reason: 'This graph does not evaluate permission to start execution.' },
      { field: 'project.doctor', reason: 'This view does not run local integrity diagnostics.' },
    ],
  }, new Date(graph.read_at_unix_ms).toISOString().replace(/\.\d{3}Z$/, 'Z'));
}
