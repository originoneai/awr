# Product-language scanner lexicon

The Observer visible-text scanner lives in `tools/observer/src/i18n/human-language.ts` (`FORBIDDEN_VISIBLE_PATTERNS` and `ALLOWED_PRODUCT_WORDS`) and runs through `tools/observer/src/lib/human-language-guard.ts`. Every page test shares this lexicon to keep rendered copy, tooltips, and accessibility labels in product language.

## Allowed product words

Each allowed word is recorded in `ALLOWED_PRODUCT_WORDS` with its rationale, a product-copy example, and a related internal counterexample that stays flagged:

- `绑定` (`zh-CN`): ordinary Chinese verb used in the prototype Administration Settings copy (`测试结果：绑定到代码版本`) to explain that test results correspond to a code version. Internal field names such as `current_binding` and `binding_id`, operation names such as `binding.verify`, and hashes such as `sha256:...` remain flagged.
- `completion` (`en-US`): ordinary English noun describing whether work is finished (for example, `The Agent reported completion, but did not submit it for review, so it is not complete` or `No completion record yet`). Internal fields and raw status codes such as `selected_completion_id`, `completion_not_recorded`, `completion_policy`, and `completion.inspect` remain flagged.

## Prototype static copy fixture

`tools/observer/src/test-fixtures/prototype-static-copy.json` holds verbatim static copy across every product page of the panorama prototype (`shell`, `home`, `projects`, `projectOverview`, `projectPlan`, `projectDeps`, `projectDelivery`, `projectKnowledge`, `task`, `person`, `goal`, `doc`, `update`, `inbox`, `lessons`, `members`, `activity`, `settings`, and `takeover`). The guard test suite scans every entry through both `findForbiddenVisibleText` and DOM-level `findHumanLanguageViolations` and verifies that real internal tokens injected alongside prototype copy stay caught.

## Code and tests

- Lexicon and rationales: `tools/observer/src/i18n/human-language.ts`
- Scanner entry points: `tools/observer/src/lib/human-language-guard.ts`
- Fixture and counterexample tests: `tools/observer/src/lib/human-language-guard.test.ts` and `tools/observer/src/test-fixtures/prototype-static-copy.json`
- Administration Settings scan without exclusions: `tools/observer/src/pages/SettingsPage.test.tsx`
