import { DEMO_PROJECT_OVERVIEW, DEMO_WORK_INBOX, DEMO_WORK_ITEMS, DEMO_WORK_NEXT, demoWorkSnapshot } from '../test-utils';
import ACTIVITY_DEVELOPMENT from './activity/observer-activity-development.json';
import ACTIVITY_REQUESTS from './activity/observer-activity-requests.json';
import OVERVIEW_ROWS from './overview/overview-works-run09.json';
import PROGRESS from './work-state/progress-reports-run09.json';
import SNAPSHOTS from './work-state/snapshots-run09.json';

/**
 * The demo data of the page tests with the real values of a rehearsal run laid over it. The works are named the
 * way the service named them (a slug for a title, a long workstream id), and the checkpoints are the ones their
 * reporters really wrote: machine names, receipt ids and commit hashes inside ordinary sentences. The human-language
 * ratchet reads pages against this, because a demo sentence never contains such things.
 */
export const REAL_API_WORK = 'issue-filter-api';
export const REAL_PAGE_WORK = 'issue-filter-page';

/** The same work at two moments of the run: waiting for its review, and delivered. */
export type RealMoment = 'in-review' | 'delivered';

const [API_ROW, PAGE_ROW] = OVERVIEW_ROWS.works;
const STREAM = OVERVIEW_ROWS.workstreams[0];

export function realShapedOverview() {
  return {
    ...DEMO_PROJECT_OVERVIEW,
    works: DEMO_PROJECT_OVERVIEW.works.map((work, index) => ({ ...work, ...[API_ROW, PAGE_ROW][index] })),
    workstreams: OVERVIEW_ROWS.workstreams,
    coverage: {
      ...DEMO_PROJECT_OVERVIEW.coverage,
      streams: [{
        workstream_id: STREAM.id,
        status: 'succeeded',
        works_enumerated: 2,
        works_total: 2,
        truncated: false,
        next_cursor: null,
        read_at_unix_ms: 1761840000000,
      }],
      works_total: 2,
    },
  };
}

/** The observer's own activity replies of the run, both kinds in one list: real operation names, a raw actor id, an unknown result. */
export function realShapedActivity() {
  return {
    ...ACTIVITY_DEVELOPMENT,
    data: { ...ACTIVITY_DEVELOPMENT.data, items: [...ACTIVITY_DEVELOPMENT.data.items, ...ACTIVITY_REQUESTS.data.items] },
  };
}

/** The resume entry of a real work.next reply of the run: a session id that is a ULID, and its version as a string. */
const RESUME_ENTRY = {
  next_query: {
    op: 'work.observe',
    protocol_version: 1,
    session_id: '01M69PHCX2R0Y5M588J945JDEH',
    work_id: REAL_API_WORK,
    workstream_id: STREAM.id,
  },
  session_id: '01M69PHCX2R0Y5M588J945JDEH',
  session_version: '1',
  work_id: REAL_API_WORK,
  workstream_id: STREAM.id,
};

export function realShapedWorkNext(): typeof DEMO_WORK_NEXT {
  const keys = [REAL_API_WORK, REAL_PAGE_WORK];
  return {
    ...DEMO_WORK_NEXT,
    result: {
      ...DEMO_WORK_NEXT.result,
      data: {
        ...DEMO_WORK_NEXT.result.data,
        resume: [RESUME_ENTRY] as never[],
        items: DEMO_WORK_ITEMS.map((item, index) => ({
          ...item,
          work_id: keys[index],
          title: keys[index],
          workstream_id: STREAM.id,
          next_query: { ...item.next_query, work_id: keys[index], workstream_id: STREAM.id },
        })),
      },
    },
  };
}

export function realShapedWorkInbox(): typeof DEMO_WORK_INBOX {
  return {
    ...DEMO_WORK_INBOX,
    result: {
      ...DEMO_WORK_INBOX.result,
      data: {
        ...DEMO_WORK_INBOX.result.data,
        items: [{
          item_key: 'k1',
          title: REAL_PAGE_WORK,
          title_truncated: false,
          work_id: REAL_PAGE_WORK,
          workstream_id: STREAM.id,
          contract_hash: 'abc',
          guidance: {
            code: 'intake',
            action: { op: 'work.prepare', note: 'Consume context and start', query: {} },
            because: ['responsibility allows'],
          },
          next_query: { op: 'work.prepare', protocol_version: 1, work_id: REAL_PAGE_WORK, workstream_id: STREAM.id },
        }],
      },
    },
  };
}

/**
 * A work.snapshot reply: the page work is still unreported and carries the service's long guidance for an agent; the
 * api work carries its real checkpoint at the moment asked for.
 */
export function realShapedSnapshot(workId: string, moment: RealMoment = 'in-review') {
  const snapshot = demoWorkSnapshot(workId) as unknown as { result: { data: Record<string, unknown> } };
  const isPage = workId === REAL_PAGE_WORK;
  const real = isPage ? SNAPSHOTS['api-in-progress-with-live-report'] : SNAPSHOTS[moment === 'delivered' ? 'api-delivered' : 'api-in-review-owned-by-other'];
  const progress = isPage ? null : PROGRESS[moment === 'delivered' ? 'api-delivered' : 'api-in-review-owned-by-other'];
  const data = snapshot.result.data as { observation: Record<string, unknown>; runtime: Record<string, unknown> };
  snapshot.result.data = {
    ...data,
    runtime: { ...data.runtime, ...real.runtime },
    observation: { ...data.observation, ...real.observation, progress },
    guidance: real.guidance,
    next_step: real.next_step,
  };
  return snapshot;
}

// Content reads (slice 2): real replies of the controlled content interfaces, trimmed to their
// data envelope like the fixtures above. The documents and digests are the run's own; nothing is
// invented. The wrong-digest reply is the 503 body the service answers when expected_sha256 does
// not match — an error code, never proof that the source changed.
import SOURCE_CONTENT_OK from './content/source-content-support-md-run07.json';
import SOURCE_CONTENT_WRONG_DIGEST from './content/source-content-wrong-digest-run07.json';
import ARTIFACT_SEGMENT from './content/artifact-segment-api-run09.json';

export const REAL_SUPPORT_MD_SHA256 = SOURCE_CONTENT_OK.data.sha256;
export const REAL_SUPPORT_MD_BYTES = SOURCE_CONTENT_OK.data.byte_length;
export const REAL_ARTIFACT_SHA256 = ARTIFACT_SEGMENT.data.sha256;
export const REAL_ARTIFACT_BYTES = ARTIFACT_SEGMENT.data.byte_length;

export function realSourceContentReply() {
  return { ok: true, result: { data: SOURCE_CONTENT_OK.data } };
}

export function realSourceContentWrongDigestReply() {
  return { ok: false, error: SOURCE_CONTENT_WRONG_DIGEST };
}

export function realArtifactSegmentReply() {
  return { ok: true, result: { data: ARTIFACT_SEGMENT.data } };
}
