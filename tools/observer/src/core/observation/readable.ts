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
