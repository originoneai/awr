import { rawObservationText } from './values';
import type { GroupWorksResult, WorkGroup, WorkGroupWork, WorkstreamRef } from './types';

/**
 * Group works under their workstreams in the order the service gave them. Each work lands in
 * exactly one group (the first workstream entry with its id), works of an unknown workstream
 * follow after the groups in their own order, and nothing is copied or dropped. A group is named by
 * its title, else its key, else its id.
 */
export function groupWorks<W extends WorkGroupWork>(works: readonly W[], workstreams: readonly WorkstreamRef[]): GroupWorksResult<W> {
  const groups: WorkGroup<W>[] = [];
  const titles: Record<string, string> = {};
  for (const workstream of workstreams) {
    if (Object.prototype.hasOwnProperty.call(titles, workstream.id)) continue;
    const title = rawObservationText(workstream.title).text ?? rawObservationText(workstream.external_key).text ?? workstream.id;
    titles[workstream.id] = title;
    groups.push({ id: workstream.id, title, works: works.filter((work) => work.workstream_id === workstream.id) });
  }
  const remaining = works.filter((work) => !Object.prototype.hasOwnProperty.call(titles, work.workstream_id));
  return { works: [...groups.flatMap((group) => group.works), ...remaining], groups, titles };
}
