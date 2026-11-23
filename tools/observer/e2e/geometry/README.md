# Observer E2E Geometry Modules

`../demo-organization-check.mjs` runs real-browser acceptance across all demo variants and loads per-page checks from this `e2e/geometry/` directory.

## Directory layout

- `../demo-organization-check.mjs`: starts `node server.js --demo` for each variant, registers routes (`fullRoutes`, `variantRoutes`), visits every route at `1440x900` and `390x844`, runs shared shell and product-language assertions, invokes registered page modules, and writes `report.json`.
- `shared.mjs`: shared `settle`, `parseMetricNumber`, and `createEqual` helpers (excluded from page-module registration).
- `<page>.mjs`: one module per page (`workbench`, `project-plan`, `project-acceptance`, `project-knowledge`, `activity`, `inbox`, `projects`, `members`).
- `../geometry.test.mjs`: unit and fault-injection tests for registry synchronization, entrypoint invocation, route action validation, and every page check function.

## Adding a new page check

Adding a page's geometry check requires exactly two steps:

1. **Create one module file** `tools/observer/e2e/geometry/<page>.mjs` exporting a default descriptor (plus named exports for direct testing):

```js
import { createEqual } from './shared.mjs';

export async function exampleGeometry(page) {
  await page.getByTestId('page-example').waitFor();
  return page.evaluate(() => ({ /* computed styles and DOM counts */ }));
}

export function checkExampleGeometry(metric, compact) {
  const failures = [];
  const equal = createEqual(failures);
  equal(metric.header?.paddingTop, compact ? 16 : 24, 'example header top padding');
  return failures;
}

export default {
  name: 'example',
  reportKey: 'exampleGeometry',
  matches: (route) => route.name === 'example',
  measure: exampleGeometry,
  check: checkExampleGeometry,
};
```

2. **Add one registration line** `'<page>',` to `registeredPageModules` in `tools/observer/e2e/demo-organization-check.mjs`. If the page introduces a new route that is absent from `fullRoutes` or `variantRoutes`, add that route entry there as well.

### Registry enforcement

Both `e2e/geometry.test.mjs` and `e2e/demo-organization-check.mjs` verify that the `.mjs` files in `e2e/geometry/` (excluding `shared.mjs`) match `registeredPageModules` one-to-one:

- Leaving a new `<page>.mjs` unregistered, deleting a registration while its file still exists, referencing a missing file, or duplicating an entry immediately fails both `node --test e2e/geometry.test.mjs` and `node e2e/demo-organization-check.mjs`.
- `e2e/geometry.test.mjs` derives expected report keys, canonical keys, and route action keys directly from the discovered and registered modules, so adding a new page requires zero edits to hardcoded test arrays.

### Route actions and canonical records

Optional module capabilities:

- `routeActions`: map of `{ [routeName]: async (page) => { ... } }` for page-specific interactive walks during a visit.
  - **Implicit cross-variant binding:** each key binds by route `name` and runs on **every variant** that declares a route with that same `name` (for example, `'project-plan'` runs in both `full` and `single` variants).
  - **Existing route check:** every `routeActions` key must match a route `name` declared in `fullRoutes` or `variantRoutes`; unknown route names throw an error during `buildPageRegistry`.
- `canonical`: `{ reportKey, route: { name, path, run: async (page, recordCanonical) => { ... } } }` for canonical-variant oracle assertions and report records. Canonical routes run their own `route.run` directly rather than using `routeActions`.

## Running checks

```sh
cd tools/observer
npx vite build
node --test e2e/pixel-pairs.guard.test.js e2e/geometry.test.mjs
node e2e/demo-organization-check.mjs
```

Set `OBSERVER_EVIDENCE_DIR` to write screenshots and `report.json` to a chosen local directory outside Git.
