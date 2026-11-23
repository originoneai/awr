# Observer E2E Geometry Checks

`demo-organization-check.mjs` runs real-browser acceptance across all demo variants and keeps per-page geometry checks isolated under `e2e/geometry/`.

## Directory layout

- `demo-organization-check.mjs`: starts `node server.js --demo` for each variant, registers routes, visits every route at `1440x900` and `390x844`, runs shared shell and product-language assertions, invokes registered page modules, and writes `report.json`.
- `geometry/shared.mjs`: shared `settle`, `parseMetricNumber`, and `createEqual` helpers.
- `geometry/<page>.mjs`: one module per page (`workbench`, `project-plan`, `project-acceptance`, `project-knowledge`, `activity`, `inbox`, `projects`, `members`).
- `geometry.test.mjs`: unit and fault-injection tests for the registry and every page check function.

## Adding a new page check

Adding a page's geometry check takes one new module file and one registration line:

1. Create `tools/observer/e2e/geometry/<page>.mjs` exporting a default descriptor (plus named exports for direct testing):

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

Optional module capabilities:

- `routeActions`: map of `{ [routeName]: async (page) => { ... } }` for page-specific interactive walks during a visit.
- `canonical`: `{ reportKey, route: { name, path, run: async (page, recordCanonical) => { ... } } }` for canonical-variant oracle assertions and report records.

2. Add one line `'<page>',` to `registeredPageModules` in `tools/observer/e2e/demo-organization-check.mjs`. If the page introduces a new route that is currently absent from `fullRoutes` or `variantRoutes`, add the route entry there as well.

## Running checks

```sh
cd tools/observer
npx vite build
node --test e2e/geometry.test.mjs
node e2e/demo-organization-check.mjs
```

Set `OBSERVER_EVIDENCE_DIR` to write screenshots and `report.json` to a chosen local directory outside Git.
