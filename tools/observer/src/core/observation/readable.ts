import { CHECK_VOCABULARY, OPERATION_VOCABULARY } from '../../i18n/human-language';

const UUID = /\b[0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}\b/gi;
/** Git object ids and sha256 digests. */
const LONG_HEX = /\b[0-9a-f]{40,64}\b/gi;
const ULID = /\b[0-9A-HJKMNP-TV-Z]{26}\b/g;

function shorten(id: string): string {
  return `${id.slice(0, 8)}…`;
}

/**
 * A sentence a reporter wrote, with its long machine identifiers (UUIDs, git object ids and digests,
 * ULIDs) cut to their first eight characters so they stop drowning the words around them. Nothing
 * else changes, and the original sentence stays in the technical details.
 */
export function abbreviateIds(text: string): string {
  return text.replace(UUID, shorten).replace(LONG_HEX, shorten).replace(ULID, shorten);
}

/** A dotted or snake_case machine name inside a sentence ("delivery.finalize", "local_git.manifest"). */
const MACHINE_NAME = /\b[a-z][a-z0-9_]*(?:\.[a-z][a-z0-9_]*)+\b|\b[a-z]+_[a-z_]+\b/g;

function knownName(name: string, zh: boolean): string | null {
  if (Object.prototype.hasOwnProperty.call(OPERATION_VOCABULARY, name)) {
    const entry = OPERATION_VOCABULARY[name];
    return zh ? entry.zh : entry.en;
  }
  if (Object.prototype.hasOwnProperty.call(CHECK_VOCABULARY, name)) return CHECK_VOCABULARY[name][zh ? 0 : 1];
  return null;
}

/**
 * A sentence a reporter wrote, made readable: the machine names the shared vocabularies know are said in
 * their plain words ("delivery.finalize" as "Finalize delivery"), long ids are cut as `abbreviateIds`
 * does, and everything else, machine names nobody has a word for included, stays as written. The
 * original sentence stays in the technical details.
 */
export function readableSentence(text: string, language: string): string {
  const zh = language.toLowerCase().startsWith('zh');
  return abbreviateIds(text.replace(MACHINE_NAME, (name) => knownName(name, zh) ?? name));
}
