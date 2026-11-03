import { describe, expect, it } from 'vitest';
import { readableSourceRef } from './lib/queries';

/**
 * The refusal rules of the bridge itself (tools/inspector/team-bridge.js): a source read needs a
 * controlled project-relative path (no leading slash, no `..` anywhere, no backslash, no colon,
 * no control characters, 1 to 512 characters) and a 64-digit lowercase hexadecimal digest. The
 * page offers no entry and sends nothing for a reference that fails any rule.
 */
describe("readableSourceRef: the bridge's own rules", () => {
  const HEX64 = 'c101fd5c70986de856c0bd30d2da271b71074408acbec05fabc9fa095e1b80ac';

  it.each([
    ['a project-relative path', 'docs/support.md', true],
    ['a leading slash', '/etc/passwd', false],
    ['dot-dot inside a name', 'docs/../secrets.txt', false],
    ['dot-dot as the whole step', '../support.md', false],
    ['a backslash', 'docs\\support.md', false],
    ['a colon (a drive or scheme)', 'docs:support.md', false],
    ['a control character', 'docs/support\x01.md', false],
    ['an empty path', '', false],
    ['512 characters (the bridge limit)', `docs/${'a'.repeat(508 - 4)}.md`, true],
    ['513 characters (past the limit)', `docs/${'a'.repeat(509 - 4)}.md`, false],
  ])('path %s', (_name, path, expected) => {
    expect(readableSourceRef(path, HEX64)).toBe(expected);
  });

  it.each([
    ['64 lowercase hex', HEX64, true],
    ['upper-case hex', HEX64.toUpperCase(), false],
    ['63 characters', HEX64.slice(1), false],
    ['65 characters', `${HEX64}f`, false],
    ['non-hex characters', `g${HEX64.slice(1)}`, false],
    ['not a string', null, false],
  ])('digest %s', (_name, sha256, expected) => {
    expect(readableSourceRef('docs/support.md', sha256 as string | null)).toBe(expected);
  });

  it('refuses a reference without a path', () => {
    expect(readableSourceRef(null, HEX64)).toBe(false);
    expect(readableSourceRef(undefined, HEX64)).toBe(false);
  });
});
