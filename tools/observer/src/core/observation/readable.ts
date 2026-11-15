import { CHECK_VOCABULARY, OPERATION_VOCABULARY, STATUS_LABELS } from '../../i18n/human-language';

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
 * else changes. Original reports remain in retained service history.
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
 * Present the business sentence without protocol names or machine identifiers.
 * The original report remains in the service's retained project history.
 */
export function readableSentence(text: string, language: string): string {
  const zh = language.toLowerCase().startsWith('zh');
  const reference = zh ? '相关记录' : 'the related record';
  return text
    .replace(UUID, reference)
    .replace(LONG_HEX, reference)
    .replace(ULID, reference)
    .replace(/\bAWR-[A-Z0-9]+(?:-[A-Z0-9]+)+\b/g, reference)
    .replace(/\bsha256:[a-f0-9]+/gi, reference)
    .replace(MACHINE_NAME, (name) => knownName(name, zh) ?? (zh ? '相关操作' : 'the related operation'))
    .replace(/\b[A-Z][A-Za-z]*(?:Error|Exception)\b/g, zh ? '执行问题' : 'an execution problem');
}

const INTERNAL_FIELDS = new Set([
  'actor_id', 'client_id', 'session_id', 'execution_id', 'claim_id', 'receipt_id', 'credential_id',
  'source_snapshot_id', 'workstream_id', 'decision_id', 'request_id', 'request_key', 'contract_hash',
  'baseline_digest', 'candidate_id', 'candidate_digest', 'project_revision', 'decision_version', 'allowed_responder_person_ids',
]);
const DIAGNOSTIC_FIELDS = new Set(['status', 'state', 'reason', 'actor', 'client', 'source', 'path']);
const ERROR_CODE = /\b(?:Forbidden|Unauthenticated|SessionExpired|CursorExpired|ContextIncomplete|ResponseTooLarge|InvalidInput|BadGateway)\b/i;
const INTERNAL_TOKEN = /\b(?:client|actor|session|execution|claim|receipt|credential|snapshot|workstream|decision|request|agent)[_-](?=[a-z0-9_-]*\d)[a-z0-9_-]+\b/i;
const SYSTEM_PATH = /(?:[A-Za-z]:\\|\/(?:Users|home|var|private|tmp|opt|etc|srv)\/|~\/|\b(?:crates|tools|src|target|\.local)\/)[^\s<>"']+/;
const OPAQUE_URL_COMPONENT = new RegExp([UUID.source, LONG_HEX.source, ULID.source, INTERNAL_TOKEN.source].join('|'), 'i');
const SENSITIVE_FIELD = /(?:^payload$|(?:authorization|cookie|credential|password|passwd|passphrase|secret|token|(?:api|private|access|signing)[_-]?key)\d*(?:$|[_-]))/i;

function internalField(field: string): boolean {
  return INTERNAL_FIELDS.has(field) || /_(?:id|hash|digest)$/.test(field);
}

function diagnosticValue(field: string, value: string): boolean {
  const scalar = value.replace(/^["']|["']$/g, '').replace(/[.!?]+$/, '');
  return ERROR_CODE.test(scalar) || INTERNAL_TOKEN.test(scalar) || SYSTEM_PATH.test(scalar)
    || ['status', 'state'].includes(field) && Object.hasOwn(STATUS_LABELS, scalar.toLowerCase());
}

function removeDiagnosticAssignments(text: string): string {
  return text.replace(/["']?\b([a-z][a-z0-9_]*)["']?\s*([:：=])\s*("[^"\n]*"|'[^'\n]*'|[^\s,;]+)/gi,
    (original, rawField: string, separator: string, value: string) => {
      const field = rawField.toLowerCase();
      return internalField(field) || DIAGNOSTIC_FIELDS.has(field) && (separator === '=' || diagnosticValue(field, value))
        ? '' : original;
    });
}

function businessUrl(text: string, reference: string): string {
  let url: URL;
  try { url = new URL(text); } catch { return reference; }
  if (/^(?:(?:[a-z0-9-]+\.)*localhost|127(?:\.\d+){3}|\[::1\]|0\.0\.0\.0)$/.test(url.hostname.replace(/\.$/, ''))
    || url.username || url.password || /^\/v\d+\/(?:projects|web)(?:\/|$)/.test(url.pathname)) return reference;
  let pathname: string;
  try { pathname = decodeURIComponent(url.pathname); } catch { return reference; }
  if (OPAQUE_URL_COMPONENT.test(url.hostname) || OPAQUE_URL_COMPONENT.test(pathname) || SYSTEM_PATH.test(pathname)) return reference;
  let changed = false;
  for (const [rawField, value] of [...url.searchParams]) {
    const field = rawField.toLowerCase();
    if (SENSITIVE_FIELD.test(field)) return reference;
    if (internalField(field) || DIAGNOSTIC_FIELDS.has(field) && diagnosticValue(field, value)) {
      url.searchParams.delete(rawField);
      changed = true;
    } else if (/[_-]|[a-z][A-Z]/.test(rawField)
      || OPAQUE_URL_COMPONENT.test(field) || OPAQUE_URL_COMPONENT.test(value)) {
      return reference;
    }
  }
  let fragment: string;
  try { fragment = decodeURIComponent(url.hash); } catch { return reference; }
  if (removeDiagnosticAssignments(fragment) !== fragment || OPAQUE_URL_COMPONENT.test(fragment)) {
    url.hash = ''; changed = true;
  }
  return changed ? url.toString() : text;
}

/** Preserve business prose and public references while removing internal diagnostic values. */
export function readableNarrative(text: string, language: string): string | null {
  // Diagnostic payloads stay in protected service history. A partial scrub of
  // structured content cannot establish that the remaining values are prose.
  if (/[{}]|```|\[\s*(?:["'\[\d]|true\b|false\b|null\b)/i.test(text)
    || [...text.matchAll(/["']?\b([a-z][a-z0-9_-]*)["']?\s*[:：=]/gi)]
      .some(([, field]) => SENSITIVE_FIELD.test(field))
    || [...removeDiagnosticAssignments(text.replace(/https?:\/\/[^\s()]+/gi, ''))
      .matchAll(/["']?\b([a-z][a-z0-9_-]*)["']?\s*([:：=])/gi)]
      .some(([, field, separator]) => separator === '=' || /[_-]|[a-z][A-Z]/.test(field))
    || /\bBearer\s+\S+|-----BEGIN [A-Z ]*PRIVATE KEY-----|^\s*(?:Traceback\b|at\s+\S+\s*\([^\n]+:\d+)/im.test(text)) return null;
  const zh = language.toLowerCase().startsWith('zh');
  const record = zh ? '相关记录' : 'the related record';
  const file = zh ? '相关文件' : 'the related file';
  const result = text.split(/(https?:\/\/[^\s()]+)/gi).map((part) => {
    if (/^https?:\/\//i.test(part)) return businessUrl(part, record);
    const redacted = removeDiagnosticAssignments(part)
      .replace(new RegExp(SYSTEM_PATH.source, 'g'), file)
      .replace(new RegExp(INTERNAL_TOKEN.source, 'gi'), record)
      .replace(new RegExp(ERROR_CODE.source, 'gi'), zh ? '处理问题' : 'a processing issue');
    return readableSentence(redacted, language);
  }).join('').trim();
  const content = result.replaceAll(record, '').replaceAll(file, '').replace(/[^\p{L}\p{N}]+/gu, '');
  return content ? result : null;
}
