import { ACTION_VOCABULARY, ARTIFACT_VOCABULARY, CHECK_VOCABULARY, FORBIDDEN_VISIBLE_PATTERNS, humanText } from '../../i18n/human-language';
import { statementText } from './values';
import type { ActionWordsResult, ActorAliasEntry, ActorAliasResult, ArtifactLabelResult, CheckLabelResult, DocumentLabelResult, WorkLabelResult } from './types';

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

/**
 * The name to show for a check or test a reporter listed: a name that is plain words, as written; the
 * shared words for a machine name the vocabulary knows ("local_git.manifest"); and "Check N", N being
 * its place in the reporter's list, for any other name that reads as a machine name or when there is
 * no name. The raw name is always returned for technical details.
 */
export function checkLabel(name: unknown, options: { language: string; position: number }): CheckLabelResult {
  const raw = statementText(name);
  const numbered = humanText('checkNumbered', options.language).replace('{{count}}', String(options.position));
  if (raw == null) return { kind: 'numbered', text: numbered, technical: { raw: null } };
  if (Object.prototype.hasOwnProperty.call(CHECK_VOCABULARY, raw)) {
    const [zh, en] = CHECK_VOCABULARY[raw];
    return { kind: 'known', text: options.language.toLowerCase().startsWith('zh') ? zh : en, technical: { raw } };
  }
  if (FORBIDDEN_VISIBLE_PATTERNS.some(({ pattern }) => pattern.test(raw))) return { kind: 'numbered', text: numbered, technical: { raw } };
  return { kind: 'named', text: raw, technical: { raw } };
}

/**
 * A label that is one word holding a slash, a colon or a backslash (a branch, a path, a URL), or a
 * bare hash, names an engineering object. The patterns of the human-language guard do not see it:
 * a branch name has no dot, no underscore and no engineering word.
 */
function readsAsReference(label: string): boolean {
  return /^\S+$/.test(label) && (/[/:\\]/.test(label) || /^[0-9a-f]{12,}$/i.test(label));
}

/**
 * The name to show for a thing the reporter says was produced: the shared words for a label the
 * vocabulary knows ("development branch" as 开发分支, whatever its capitals and spacing); any other
 * plain label as written; and "Output N", N being its place in the reporter's list, for a label that
 * reads as an identifier or a reference, or when there is none. The raw label is always returned for
 * technical details; the artifact's reference never reaches the default view.
 */
export function artifactLabel(label: unknown, options: { language: string; position: number }): ArtifactLabelResult {
  const raw = statementText(label);
  const numbered = humanText('artifactNumbered', options.language).replace('{{count}}', String(options.position));
  if (raw == null) return { kind: 'numbered', text: numbered, technical: { raw: null } };
  const key = raw.replace(/\s+/g, ' ').toLowerCase();
  if (Object.prototype.hasOwnProperty.call(ARTIFACT_VOCABULARY, key)) {
    const [zh, en] = ARTIFACT_VOCABULARY[key];
    return { kind: 'known', text: options.language.toLowerCase().startsWith('zh') ? zh : en, technical: { raw } };
  }
  if (FORBIDDEN_VISIBLE_PATTERNS.some(({ pattern }) => pattern.test(raw)) || readsAsReference(raw)) return { kind: 'numbered', text: numbered, technical: { raw } };
  return { kind: 'named', text: raw, technical: { raw } };
}

/**
 * The name to show for a document known by its path: the file name without its directories and its
 * extension, with hyphens and underscores read as spaces ("docs/specs/issue-filter-api.md" as
 * "issue filter api"). A path with no usable name, or one whose name the human-language guard would
 * flag, reads "Document N", N being its place in the list. The path itself is always returned for
 * the technical details.
 */
export function documentLabel(path: unknown, options: { language: string; position: number }): DocumentLabelResult {
  const raw = statementText(path);
  const numbered = humanText('documentNumbered', options.language).replace('{{count}}', String(options.position));
  if (raw == null) return { kind: 'numbered', text: numbered, technical: { raw: null } };
  const file = raw.split(/[\\/]/).filter((part) => part !== '').pop() ?? '';
  const stem = file.replace(/(?<=.)\.[A-Za-z0-9]{1,8}$/, '');
  const name = stem.replace(/[-_]+/g, ' ').replace(/\s+/g, ' ').trim();
  if (name === '' || FORBIDDEN_VISIBLE_PATTERNS.some(({ pattern }) => pattern.test(name))) {
    return { kind: 'numbered', text: numbered, technical: { raw } };
  }
  return { kind: 'named', text: name, technical: { raw } };
}

/**
 * The operations a member or an agent may be allowed to do, in plain words, each once and in the order
 * the service listed them. A name the vocabulary has no words for is counted, never shown; the exact
 * names come back for the technical details. Anything that is not a list of names reads as none.
 */
export function actionWords(actions: unknown, options: { language: string }): ActionWordsResult {
  const names = Array.isArray(actions) ? Array.from(new Set(actions.flatMap((action) => {
    const name = statementText(action);
    return name == null ? [] : [name];
  }))) : [];
  const zh = options.language.toLowerCase().startsWith('zh');
  const shown: string[] = [];
  let other = 0;
  for (const name of names) {
    if (Object.prototype.hasOwnProperty.call(ACTION_VOCABULARY, name)) shown.push(ACTION_VOCABULARY[name][zh ? 0 : 1]);
    else other += 1;
  }
  return { shown, other, technical: { raw: names } };
}
