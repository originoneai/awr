# Project organization directory

The directory associates a registered project and its existing responsibility members with an organization and departments. Each project owns a versioned directory. Explicit references support grouping across projects, and each member reference carries both its project and person identity.

## Authority

Directory reads require a current project-scoped `inspect` delegation for Agents and read grants covering the project's registered workstreams. Management requires the `directory.manage_project` business action and management grants covering those workstreams. Project administrators with the supervisor or administrator business duty can receive this action.

Agents receive management through the explicit `manage_directory` authorization action at project scope. The operator provisioning entry accepts this action. Its product action is limited to directory management. Existing member administration has its own authorization contract.

The service rechecks membership, credential, delegation and scope inside the operation transaction. Mutations serialize on the existing project writer lock. Organization and department names are display labels. Grouping uses their explicit references, while person identity uses `(project_id, person_id)`.

## Agent and HTTP entries

| Agent tool | HTTP POST path | Required input |
| --- | --- | --- |
| `awr_team_directory_read` | `/v1/projects/{project}/directory/read` | `protocol_version` |
| `awr_team_directory_preview` | `/v1/projects/{project}/directory/preview` | `protocol_version`, `plan` |
| `awr_team_directory_apply` | `/v1/projects/{project}/directory/apply` | `protocol_version`, `request_id`, `expected_version`, `expected_plan_digest`, `plan` |
| `awr_team_directory_outcome` | `/v1/projects/{project}/directory/outcome` | `protocol_version`, original `request_id` |

Every entry uses protocol version `1`. Authentication selects the tenant, project, actor and client. The body supplies the proposed relationships.

```json
{
  "organization": { "id": "example", "name": "Example company" },
  "departments": [
    { "id": "engineering", "name": "Engineering" }
  ],
  "project_department_ids": ["engineering"],
  "members": [
    { "person_id": "existing-project-member", "department_ids": ["engineering"] }
  ]
}
```

Plans contain at most 128 departments and 256 members within a 64 KiB bound. Member references must resolve to active persons in the bound project. References within a plan are unique and resolve to declared departments. Array order is normalized for hashing.

## Conditional changes and recovery

1. Read the current directory and prepare the proposed relationships.
2. Preview the plan. The result returns `expected_version`, `plan_digest`, and the before/after relationships.
3. Apply that plan with the returned version and digest under a stable request identifier.
4. Retain the confirmed receipt. An uncertain response requires querying the original identifier through the outcome entry, then retrying the original intent if its result remains absent.
5. A version conflict requires a fresh read and preview. A changed intent uses a new request identifier after the earlier outcome has been resolved.

The initial version is `0`. Each confirmed apply advances the directory version. Exact replay returns the historical receipt with `state_basis` set to `at_commit`. Receipt identity includes the authenticated actor and client. Current authority is checked again when replaying or inspecting an outcome.

A clearing plan uses `organization: null` and empty department, project-department and member arrays. Renames preserve explicit references. Removed or departed members remain recognizable in the saved relationships, with current availability and display-name availability reported separately.

## Persistence and observation

Schema 62 adds the current directory and append-only change receipts. An apply records its plan, version, authenticated authorization provenance, project revision and operations audit in one transaction. Reconnecting through a new service/store instance reads the same current directory and original receipt.

Observation clients use project-qualified references to drill back to project facts. Product views present organization, department and member names with relationship availability. Version tokens, authorization bindings and diagnostic fields belong in the protected service records.

## Focused verification

The domain tests cover shape, bounds, reference validation and order-independent hashing. PostgreSQL tests cover conditional writes, concurrent attempts, exact replay, recovery, revoked authority and project-qualified identities. Native MCP and HTTP tests exercise Agent delegation and the shared service entry points.

```bash
cargo test -p awr-team --lib directory --locked
cargo test -p awr-team-pg --features pg-tests --test pg_directory --locked -- --test-threads=1
cargo test -p awr-server --features pg-tests --test directory_mcp --locked -- --test-threads=1
```

The PostgreSQL fixtures use their configured loopback test server and create isolated process-owned databases.
