# Icons

The observation workspace takes every icon from one registry: `tools/observer/src/components/icons/prototype-icons.tsx`. The prototype is the acceptance standard for each glyph, so the registry rebuilds the prototype's own `ICONS` table (panorama `src3/app.js`) through lucide's `createLucideIcon` factory, with the node data copied verbatim.

## Rules

- Import icons only from the registry file. Components and pages never import an icon package directly; a guard test fails on any import of `lucide-react` or `@ant-design/icons` outside the registry.
- A new icon is first looked up in `PROTOTYPE_ICONS`, which is keyed by the prototype's own names and kept complete by a fixture test (`src/test-fixtures/prototype-icons.json`, generated from the prototype table). The fixture test also pins the svg parameters: viewBox `0 0 24 24`, `fill: none`, `stroke: currentColor`, `stroke-width: 1.5`, round caps and joins, 16px default box. These defaults live in the registry wrapper, so a plain `<SomeIcon />` already draws like the prototype.
- A glyph with no synonym in the prototype table goes into the registry's app-only section, with node data copied from the installed `lucide-react` and a one-line comment naming its use. The current app-only set: `chevron-down` (scope-switcher caret), `arrow-left` (detail back navigation), `stamp` (verified-digest seal), `refresh-cw` (project map reload), `triangle-alert` (warning notices).
- Sizes and spacing stay with the usage site: the prototype uses 16px by default, 14px chevrons in breadcrumbs and tables, and 18px for the shell bell; one stroke width everywhere.

## Why a registry

The prototype harvests a lucide snapshot older than the installed package, so same-name imports drift: the house gained a door, the search lens grew to `r8`, `users` was re-rounded, `bot`/`copy`/`unlock` became `rect` nodes, the circle-check arm changed. The registry keeps the page equal to the prototype and makes the equality testable instead of eye-measured.
