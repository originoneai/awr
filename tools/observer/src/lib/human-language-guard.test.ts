import { describe, expect, it } from 'vitest';
import { defaultVisibleText, findForbiddenVisibleText } from './human-language-guard';

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
