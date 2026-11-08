# Delivery route templates

Development on `main` adds an administrator-maintained registry for reusing
provider-neutral delivery configuration within one workstream. This registry
does not create a connector for another task or grant repository, execution,
review or access-management permissions. Per-task reuse is a separate operation.

Use `awr_team_command` with `op: delivery.route.configure` and the current
`work.prepare` header. Its arguments are:

| Argument | Meaning |
| --- | --- |
| `source_snapshot_id` | The snapshot in the prepared header. |
| `template_id` | A stable, project-unique template name. |
| `expected_template_version` | `0` for creation; the recorded version for an update. |
| `source_connector_id` | An existing connector on the prepared task. |
| `source_connector_version` | The exact connector version being exposed. |
| `enabled` | Whether this template is offered for future reuse. |

Configuration requires eligible human/system project-management authority and
management access to the workstream. The source must be an enabled system
adapter connector in the current coordinator epoch. A template cannot move
between workstreams. Keep the original `request_id` after a missing response;
query `delivery.neutral.outcome` before retrying it.

Members discover templates through `awr_team_query` with `op: delivery.routes`
and a readable `work_id`. Optional `limit`, `cursor` and `max_context_bytes`
keep the result bounded. Discovery omits worker identities and credentials.
It returns registration metadata, not worker health, verified Git contents or
permission to perform an effect. Connector changes, revocation or source
ownership changes can remove a template from a later read.

Disabling a template stops future reuse and remains possible after its source
connector changes. It does not silently reconfigure existing task mappings.
Those mappings, their versions and their existing authority checks remain
independent. Report-only task completion retains its current behavior.

The registry requires Team PostgreSQL schema 60. Use the normal owner-side
migration and application-role bootstrap procedure before starting a matching
Team server. Older schemas fail the schema gate; never migrate a frozen
acceptance environment in place. The published personal CLI/MCP package does
not provide an authenticated Team service.
