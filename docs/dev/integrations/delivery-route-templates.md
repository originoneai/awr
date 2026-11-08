# Delivery route templates

Development on `main` adds an administrator-maintained registry for reusing
provider-neutral delivery configuration within one workstream. This registry
does not grant repository, execution, review or access-management permissions.
Supervisors can select an authorized template for an unconfigured task before
assignment or self-claim.

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

## Select a route for a new task

For work that needs repository integration, discover `delivery.routes` for the
target task. Choose one template from that response and call
`delivery.route.reuse` with a fresh `work.prepare` header and these arguments:

| Argument | Meaning |
| --- | --- |
| `source_snapshot_id` | The target task's prepared source snapshot. |
| `template_id` | The selected authorized template. |
| `template_version` | Its exact current version from discovery. |
| `source_connector_version` | The source connector version from the same item. |
| `connector_id` | A new stable, project-unique name for this task's mapping. |

Reuse requires current `work.assign` authority and workstream write access,
including an appropriate live delegation for an Agent. It creates one independent
per-task mapping atomically, using the registered worker and resource without
disclosing or copying credentials. It does not assign, claim, execute, approve
or deliver the task. Bindings, review and integration still use their existing
gates.

An assigned, claimed, executed, reviewed or completed task, or any existing
mapping, is refused. Manage existing mappings through their original explicit
administration path. Competing new selections cannot overwrite each other.
Disabling or changing a template stops new selections; it does not retroactively
rewrite mappings already committed. A retained source locator is copied only
when its version matches the selected connector.

Use the same `request_id` and mapping name after an uncertain response and query
`delivery.neutral.outcome` before an exact retry. A committed receipt is historical
and does not prove the template or task is still current. Report-only work does
not need a route, and a Git revision or check list alone does not require one.

The registry requires Team PostgreSQL schema 60. Use the normal owner-side
migration and application-role bootstrap procedure before starting a matching
Team server. Older schemas fail the schema gate; never migrate a frozen
acceptance environment in place. The published personal CLI/MCP package does
not provide an authenticated Team service.
