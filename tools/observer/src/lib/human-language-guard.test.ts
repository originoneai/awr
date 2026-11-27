import { describe, expect, it } from 'vitest';
import { ALLOWED_PRODUCT_WORDS, FORBIDDEN_VISIBLE_PATTERNS } from '../i18n/human-language';
import PROTOTYPE_STATIC_COPY from '../test-fixtures/prototype-static-copy.json';
import {
  defaultVisibleText,
  findForbiddenVisibleText,
  findHumanLanguageViolations,
  findIdentifierText,
  IDENTIFIER_PATTERN_IDS,
  visibleTextParts,
} from './human-language-guard';

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

  it('includes diagnostic disclosures and leaves the page itself untouched', () => {
    const root = page('<p>Ready for review</p><div data-testid="technical-details"><span>work_id</span><pre>01M6AVQ870N7MBRJ5MPCN79T55</pre></div><p>Waiting</p>');
    expect(defaultVisibleText(root)).toBe('Ready for review\nwork_id\n01M6AVQ870N7MBRJ5MPCN79T55\nWaiting');
    expect(findForbiddenVisibleText(defaultVisibleText(root)).map((entry) => entry.id)).toEqual(['snake_case', 'ulid']);
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

  it('takes the outermost marked element once and includes disclosures in the checks', () => {
    const root = page(
      '<p data-reporter-text>Outer <span data-reporter-text>inner</span> end</p>'
      + '<div data-testid="technical-details"><p data-reporter-text>raw_name</p><span>work_id</span></div>',
    );
    expect(visibleTextParts(root)).toEqual({ own: 'work_id', reporter: 'Outer\ninner\nend\nraw_name' });
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
  it('checks tooltip and accessibility text for hidden diagnostic information', () => {
    const root = page('<button title="request_id" aria-label="execution.prepare">Open</button>');
    expect(findHumanLanguageViolations(root).map(entry => entry.id)).toContain('snake_case');
    expect(findHumanLanguageViolations(root).map(entry => entry.id)).toContain('operation_name');
  });

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


describe('structured diagnostics stay outside all product text', () => {
  it('limits the connection-address exception to the supported endpoint without credential fields', () => {
    const root = page('<code data-agent-connection-address>http://127.0.0.1:9000/v1/projects/demo/mcp</code>');
    expect(findHumanLanguageViolations(root)).toEqual([]);
    root.firstElementChild!.textContent += '?token=synthetic-connection-value';
    expect(findHumanLanguageViolations(root).map((entry) => entry.id)).toContain('diagnostic_content');
  });

  it.each(['', ' data-reporter-text'])('checks structured content even with reporter marking %s', (mark) => {
    const root = document.createElement('div');
    const paragraph = document.createElement('p');
    if (mark) paragraph.setAttribute('data-reporter-text', '');
    paragraph.textContent = 'client[secret]=synthetic-guard-value';
    root.append(paragraph);
    expect(findHumanLanguageViolations(root).map((entry) => entry.id)).toContain('diagnostic_content');
    paragraph.textContent = '{"reason":"synthetic-guard-value"}';
    expect(findHumanLanguageViolations(root).map((entry) => entry.id)).toContain('diagnostic_content');
  });

  it('checks tooltip and accessibility payloads using the same projection rules', () => {
    const root = page('<button>View progress</button>');
    root.firstElementChild!.setAttribute('aria-label', 'client[secret]=synthetic-accessible-value');
    expect(findHumanLanguageViolations(root).map((entry) => entry.id)).toContain('diagnostic_content');
    expect(findHumanLanguageViolations(page('<p data-reporter-text>Review https://example.com/guide?topic=release</p>'))).toEqual([]);
  });
});


describe('colon-form protocol content', () => {
  it.each(['reason: database unavailable', 'status: custom failure', 'state: degraded', 'path: private route'])(
    'checks reporter prose for %s', (text) => {
      const root = document.createElement('div');
      const paragraph = document.createElement('p');
      paragraph.setAttribute('data-reporter-text', '');
      paragraph.textContent = text;
      root.append(paragraph);
      expect(findHumanLanguageViolations(root).map((item) => item.id)).toContain('diagnostic_content');
    },
  );
});

describe('allowed product words and internal counterexamples', () => {
  it('documents a rationale and verified internal counterexample for every allowed word', () => {
    expect(ALLOWED_PRODUCT_WORDS.map((entry) => entry.word)).toEqual(['绑定', 'completion']);
    for (const entry of ALLOWED_PRODUCT_WORDS) {
      expect(entry.rationale.trim().length, `${entry.word} rationale`).toBeGreaterThan(10);
      expect(entry.example.toLowerCase(), `${entry.word} example`).toContain(entry.word.toLowerCase());
      expect(findForbiddenVisibleText(entry.word), entry.word).toEqual([]);
      expect(findForbiddenVisibleText(entry.example), entry.example).toEqual([]);

      const allowedDom = document.createElement('div');
      const allowedParagraph = document.createElement('p');
      allowedParagraph.textContent = entry.example;
      allowedParagraph.setAttribute('title', entry.example);
      allowedParagraph.setAttribute('aria-label', entry.word);
      allowedDom.append(allowedParagraph);
      expect(findHumanLanguageViolations(allowedDom), entry.word).toEqual([]);

      expect(findForbiddenVisibleText(entry.forbiddenCounterexample).length, entry.forbiddenCounterexample).toBeGreaterThan(0);
      const counterDom = document.createElement('div');
      const counterParagraph = document.createElement('p');
      counterParagraph.textContent = entry.forbiddenCounterexample;
      counterDom.append(counterParagraph);
      expect(findHumanLanguageViolations(counterDom).length, entry.forbiddenCounterexample).toBeGreaterThan(0);
    }
  });

  it.each([
    '绑定',
    '绑定到代码版本',
    'completion',
    'Completion',
    'The Agent reported completion, but did not submit it for review, so it is not complete',
    'No completion record yet',
  ])('accepts ordinary product copy "%s"', (text) => {
    expect(findForbiddenVisibleText(text)).toEqual([]);
    const root = document.createElement('div');
    const paragraph = document.createElement('p');
    paragraph.textContent = text;
    root.append(paragraph);
    expect(findHumanLanguageViolations(root)).toEqual([]);
  });

  it.each([
    // Counterexamples for 绑定
    ['current_binding', 'snake_case'],
    ['binding_id', 'snake_case'],
    ['binding.verify', 'operation_name'],
    // Counterexamples for completion
    ['selected_completion_id', 'snake_case'],
    ['completion_policy', 'snake_case'],
    ['completion_not_recorded', 'snake_case'],
    ['completion.inspect', 'operation_name'],
    // Internal identifiers
    ['01M6B189DXC1A21ZD5YQ6V7EG5', 'ulid'],
    ['3f2b8c1e-9a4d-4c3e-8f1a-2b7d6e5c4a39', 'uuid'],
    ['work_id', 'snake_case'],
    // Hashes
    ['5f7eb2fe9c563e4d355aa64a527e0aef754116c1', 'long_hex'],
    ['sha256:4f9c1234', 'sha256'],
    // Protocol fields and operation names
    ['request_id', 'snake_case'],
    ['client_id', 'snake_case'],
    ['business_role_presets', 'snake_case'],
    ['delivery.finalize', 'operation_name'],
    // Raw status codes and error classes
    ['waiting_user', 'snake_case'],
    ['ready_for_review', 'snake_case'],
    ['not_measured', 'snake_case'],
    ['not measured', 'not_measured'],
    ['ReadTimeoutError', 'error_class'],
    // Remaining engineering vocabulary stays forbidden
    ['claim', 'engineering_word'],
    ['lease', 'engineering_word'],
    ['digest', 'engineering_word'],
    ['checkpoint', 'engineering_word'],
    ['fence', 'engineering_word'],
    ['epoch', 'engineering_word'],
    ['payload', 'engineering_word'],
    ['cursor', 'engineering_word'],
    ['OAuth', 'engineering_word'],
    ['snapshot', 'engineering_word'],
    ['切片', 'engineering_copy'],
    ['占位', 'engineering_copy'],
    ['合同', 'engineering_copy'],
    ['回执', 'engineering_copy'],
    ['轮号', 'engineering_copy'],
    ['未测量', 'engineering_copy'],
    ['快照', 'engineering_copy'],
  ])('still flags internal token %s as %s', (text, expectedRule) => {
    expect(findForbiddenVisibleText(text).map((violation) => violation.id)).toContain(expectedRule);
    const root = document.createElement('div');
    const paragraph = document.createElement('p');
    paragraph.textContent = text;
    root.append(paragraph);
    expect(findHumanLanguageViolations(root).map((violation) => violation.id)).toContain(expectedRule);
  });

  it.each(['CursorExpired', 'SessionExpired', 'Forbidden', 'Unauthenticated', 'client-obs-657'])(
    'still flags raw status and error code %s in rendered product text', (code) => {
      const root = document.createElement('div');
      const paragraph = document.createElement('p');
      paragraph.textContent = code;
      root.append(paragraph);
      expect(findHumanLanguageViolations(root).map((violation) => violation.id)).toContain('diagnostic_content');
    },
  );
});

describe('prototype static copy fixture', () => {
  const EXPECTED_PAGES = [
    'shell', 'home', 'projects', 'projectOverview', 'projectPlan', 'projectDeps',
    'projectDelivery', 'projectKnowledge', 'task', 'person', 'goal', 'doc', 'update',
    'inbox', 'lessons', 'members', 'activity', 'settings', 'takeover',
  ];

  it('covers every prototype product page verbatim and passes both text and DOM scans', () => {
    const pages = PROTOTYPE_STATIC_COPY.pages as Record<string, string[]>;
    expect(Object.keys(pages).sort()).toEqual([...EXPECTED_PAGES].sort());
    expect(pages.settings).toContain('绑定到代码版本');
    expect(pages.settings).toContain('测试结果');

    for (const [pageName, phrases] of Object.entries(pages)) {
      expect(phrases.length, `${pageName} has static copy`).toBeGreaterThan(0);
      const root = document.createElement('section');
      for (const phrase of phrases) {
        expect(findForbiddenVisibleText(phrase), `${pageName}: ${phrase}`).toEqual([]);
        const item = document.createElement('p');
        item.textContent = phrase;
        item.setAttribute('title', phrase);
        item.setAttribute('aria-label', phrase);
        root.append(item);
      }
      expect(findHumanLanguageViolations(root), `${pageName} DOM violations`).toEqual([]);
    }
  });

  it('still catches real internal tokens when mixed into prototype static copy', () => {
    const root = document.createElement('section');
    for (const phrase of PROTOTYPE_STATIC_COPY.pages.settings) {
      const item = document.createElement('p');
      item.textContent = phrase;
      root.append(item);
    }
    const leaked = document.createElement('p');
    leaked.textContent = 'selected_completion_id · current_binding · sha256:4f9c1234 · waiting_user · CursorExpired';
    root.append(leaked);
    const ids = new Set(findHumanLanguageViolations(root).map((violation) => violation.id));
    expect(ids.has('snake_case')).toBe(true);
    expect(ids.has('sha256')).toBe(true);
    expect(ids.has('diagnostic_content')).toBe(true);
  });
});
