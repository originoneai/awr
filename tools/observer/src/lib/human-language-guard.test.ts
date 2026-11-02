import { describe, expect, it } from 'vitest';
import { FORBIDDEN_VISIBLE_PATTERNS } from '../i18n/human-language';
import { defaultVisibleText, findForbiddenVisibleText, findHumanLanguageViolations, findIdentifierText, IDENTIFIER_PATTERN_IDS, visibleTextParts } from './human-language-guard';

function page(html: string): HTMLElement {
  const root = document.createElement('div');
  root.innerHTML = html;
  return root;
}

describe('defaultVisibleText', () => {
  it('keeps the text of neighbouring elements apart, so a name next to a capital letter is still a name', () => {
    const root = page('<section><code>docs/support.md</code><p>Keep the support API stable.</p></section>');
    expect(defaultVisibleText(root)).toBe('docs/support.md\nKeep the support API stable.');
    expect(findForbiddenVisibleText(defaultVisibleText(root))).toEqual([{ id: 'operation_name', match: 'support.md' }]);
    // the glued text of the same page hides it: this is what the ratchet used to read
    expect(findForbiddenVisibleText(root.textContent ?? '')).toEqual([]);
  });

  it('leaves out the technical details, collapsed or expanded, and leaves the page itself untouched', () => {
    const root = page('<p>Ready for review</p><div data-testid="technical-details"><span>work_id</span><pre>01M6AVQ870N7MBRJ5MPCN79T55</pre></div><p>Waiting</p>');
    expect(defaultVisibleText(root)).toBe('Ready for review\nWaiting');
    expect(findForbiddenVisibleText(defaultVisibleText(root))).toEqual([]);
    expect(root.querySelector('[data-testid="technical-details"]')).not.toBeNull();
  });

  it('trims the text of each element and skips the blank ones', () => {
    expect(defaultVisibleText(page('<p>  one  </p><p> </p><p>\n</p><p>two</p>'))).toBe('one\ntwo');
    expect(defaultVisibleText(page(''))).toBe('');
  });
});

describe('visibleTextParts', () => {
  it('splits a page into its own words and the sentences a reporter wrote', () => {
    const root = page('<h2>Next step</h2><p data-reporter-text>Leave the page for self-claim.</p><p>Ready for review</p>');
    expect(visibleTextParts(root)).toEqual({ own: 'Next step\nReady for review', reporter: 'Leave the page for self-claim.' });
  });

  it('takes the outermost marked element once and leaves the technical details out of both parts', () => {
    const root = page(
      '<p data-reporter-text>Outer <span data-reporter-text>inner</span> end</p>'
      + '<div data-testid="technical-details"><p data-reporter-text>raw_name</p><span>work_id</span></div>',
    );
    expect(visibleTextParts(root)).toEqual({ own: '', reporter: 'Outer\ninner\nend' });
  });

  it('keeps the words around a marked element in the page\'s own part and leaves the page itself untouched', () => {
    const root = page('<p>claim <span data-reporter-text>as written</span></p>');
    expect(visibleTextParts(root)).toEqual({ own: 'claim', reporter: 'as written' });
    expect(root.querySelector('[data-reporter-text]')).not.toBeNull();
  });

  it('reads a page without a reporter as all its own', () => {
    expect(visibleTextParts(page('<p>one</p><p>two</p>'))).toEqual({ own: 'one\ntwo', reporter: '' });
  });
});

describe('findIdentifierText', () => {
  it.each([
    ['a machine name', 'local_git.manifest passed', ['snake_case', 'operation_name']],
    ['an operation name', 'ran delivery.finalize', ['operation_name']],
    ['a long identifier', 'receipt 01M6B189DXC1A21ZD5YQ6V7EG5 is confirmed', ['ulid']],
    ['a commit hash', 'origin/main is 5f7eb2fe9c563e4d355aa64a527e0aef754116c1', ['long_hex']],
    ['a digest prefix', 'content sha256:cd2a662a', ['sha256']],
    ['an error class', 'failed with ReadTimeoutError', ['error_class']],
  ])('finds %s', (_name, text, ids) => {
    expect(findIdentifierText(text).map((violation) => violation.id)).toEqual(ids);
  });

  it('knows, for every forbidden pattern, whether it is an identifier shape or a word list, so a new pattern cannot be silently exempt in a reporter\'s words', () => {
    const wordLists = FORBIDDEN_VISIBLE_PATTERNS.map((entry) => entry.id).filter((id) => !IDENTIFIER_PATTERN_IDS.has(id));
    expect(wordLists.sort()).toEqual(['engineering_copy', 'engineering_word', 'not_measured']);
    expect([...IDENTIFIER_PATTERN_IDS].every((id) => FORBIDDEN_VISIBLE_PATTERNS.some((entry) => entry.id === id))).toBe(true);
  });

  it('does not look for engineering or process words', () => {
    expect(findIdentifierText('Leave the page available for self-claim; the lease and the checkpoint are fine. 已有合同')).toEqual([]);
  });
});

describe('findHumanLanguageViolations', () => {
  it('lets a reporter\'s engineering word stand as written', () => {
    expect(findHumanLanguageViolations(page('<p data-reporter-text>Leave the page available for self-claim.</p>'))).toEqual([]);
  });

  it('finds an identifier inside a reporter\'s sentence', () => {
    const root = page('<p data-reporter-text>Receipt 01M6B189DXC1A21ZD5YQ6V7EG5 and local_git.manifest</p>');
    expect(findHumanLanguageViolations(root).map((violation) => violation.id)).toEqual(['snake_case', 'operation_name', 'ulid']);
  });

  it('holds the page\'s own words to the whole list, next to a marked sentence', () => {
    const root = page('<p>Take the claim</p><p data-reporter-text>self-claim is the reporter\'s word</p>');
    expect(findHumanLanguageViolations(root)).toEqual([{ id: 'engineering_word', match: 'claim' }]);
  });

  it('does not let a marked element excuse the words around it', () => {
    const root = page('<p>lease <span data-reporter-text>as written</span></p>');
    expect(findHumanLanguageViolations(root)).toEqual([{ id: 'engineering_word', match: 'lease' }]);
  });

  it('counts a page with nothing to find as none', () => {
    expect(findHumanLanguageViolations(page('<p>Ready for review</p><p data-reporter-text>Wait for the review.</p>'))).toEqual([]);
  });
});
