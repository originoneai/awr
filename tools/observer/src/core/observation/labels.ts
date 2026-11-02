import { FORBIDDEN_VISIBLE_PATTERNS, humanText } from '../../i18n/human-language';
import type { ActorAliasEntry, ActorAliasResult, WorkLabelResult } from './types';

function present(value: string | null | undefined): value is string {
  return typeof value === 'string' && value.length > 0;
}

/**
 * Give opaque actor ids stable, readable stand-ins ("Operator 1", "You") for one list.
 * The order is the code-unit order of the ids, so the same set of actors always gets the same
 * numbers whatever order events arrive in. The real id stays available to the caller.
 */
export function actorAliases(
  actorIds: ReadonlyArray<string | null | undefined>,
  options: { language: string; selfActorId?: string | null },
): ActorAliasResult {
  const distinct = Array.from(new Set(actorIds.filter(present))).sort((left, right) => (left < right ? -1 : left > right ? 1 : 0));
  const self = present(options.selfActorId) && distinct.includes(options.selfActorId) ? options.selfActorId : null;
  const others = distinct.filter((id) => id !== self);
  const entries: ActorAliasEntry[] = [];
  if (self) entries.push({ actorId: self, label: humanText('operatorSelf', options.language), isSelf: true });
  others.forEach((actorId, index) => {
    entries.push({ actorId, label: humanText('operatorN', options.language).replace('{{count}}', String(index + 1)), isSelf: false });
  });
  const byId = new Map(entries.map((entry) => [entry.actorId, entry.label]));
  return {
    entries,
    labelOf: (actorId) => (present(actorId) ? byId.get(actorId) ?? humanText('operatorMissing', options.language) : humanText('operatorMissing', options.language)),
  };
}

/**
 * The name to show for a work: its own readable name, "no related work", or a neutral phrase when
 * the only thing known is an opaque identifier. The raw value is always returned for technical details.
 */
export function workLabel(workId: string | null | undefined, options: { language: string }): WorkLabelResult {
  if (!present(workId)) return { kind: 'none', text: humanText('workNone', options.language), technical: { raw: null } };
  const opaque = FORBIDDEN_VISIBLE_PATTERNS.some(({ pattern }) => pattern.test(workId));
  if (opaque) return { kind: 'unnamed', text: humanText('workUnnamed', options.language), technical: { raw: workId } };
  return { kind: 'named', text: workId, technical: { raw: workId } };
}
