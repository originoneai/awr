# Project organization directory

The directory associates a registered project and its existing responsibility members with an organization and a hierarchy of departments. Each project owns a versioned directory. Explicit references support grouping across projects, and each member reference carries both its project and person identity.

Departments form a tree. A department names its parent department through an optional `parent_id`; a department without one sits at the top level. Directories recorded before the field existed read back with every department at the top level, which is the same tree. A leaf department (a department no other department names as parent) is what the product calls a group, and a project attaches under a group by listing that leaf in `project_department_ids`.

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
    { "id": "engineering", "name": "Engineering" },
    { "id": "platform", "name": "Platform", "parent_id": "engineering" },
    { "id": "runtime", "name": "Runtime", "parent_id": "platform" },
    { "id": "scheduling", "name": "Scheduling", "parent_id": "runtime" }
  ],
  "project_department_ids": ["scheduling"],
  "members": [
    { "person_id": "existing-project-member", "department_ids": ["scheduling"] }
  ]
}
```

Plans contain at most 128 departments and 256 members within a 64 KiB bound. Member references must resolve to active persons in the bound project. References within a plan are unique and resolve to declared departments. Array order is normalized for hashing.

Each department may carry one `parent_id`, absent or null for the top level. The service validates that every parent resolves to a declared department, that no parent chain cycles (including self-parenting), that the deepest chain holds at most four departments counting the department itself (a flat directory has depth 1), and that departments sharing one parent — including the top level — carry distinct names once trimmed. Names may repeat across different parents. A stored plan omits `parent_id` for top-level departments, so a flat plan serializes exactly as before the field existed and keeps its digest; an applied hierarchy changes the digest like any other relationship change. A directory recorded under the older rules may already hold two top-level departments with the same name. Reading it keeps working, while previewing that plan unchanged is refused until one of the two is renamed.

## Conditional changes and recovery

1. Read the current directory and prepare the proposed relationships.
2. Preview the plan. The result returns `expected_version`, `plan_digest`, and the before/after relationships.
3. Apply that plan with the returned version and digest under a stable request identifier.
4. Retain the confirmed receipt. An uncertain response requires querying the original identifier through the outcome entry, then retrying the original intent if its result remains absent.
5. A version conflict requires a fresh read and preview. A changed intent uses a new request identifier after the earlier outcome has been resolved.

The initial version is `0`. Each confirmed apply advances the directory version. Exact replay returns the historical receipt with `state_basis` set to `at_commit`. Receipt identity includes the authenticated actor and client. Current authority is checked again when replaying or inspecting an outcome.

A clearing plan uses `organization: null` and empty department, project-department and member arrays. Renames preserve explicit references. Removed or departed members remain recognizable in the saved relationships, with current availability and display-name availability reported separately.

## Persistence and observation

Schema 62 adds the current directory and append-only change receipts; the hierarchy needs no further migration because the whole plan, parent references included, lives in `plan_json`. An apply records its plan, version, authenticated authorization provenance, project revision and operations audit in one transaction. The audit summary of an apply reports `department_count`, `member_count`, `department_depth` (deepest chain, top level counting as 1) and `leaf_department_count` (the groups) next to the digest and directory version. Reconnecting through a new service/store instance reads the same current directory and original receipt.

Observation clients use project-qualified references to drill back to project facts. Product views present organization, department and member names with relationship availability, and read the hierarchy from the parent references in the plan: a leaf department is a group, and a project or member listing a leaf sits under that group. Version tokens, authorization bindings and diagnostic fields belong in the protected service records.

The Observer **Members & Agents** page reads `/v1/web/projects/{project}/directory/read` through its existing HttpOnly browser session and the `/api/team/directory` bridge. The service repeats the same directory authorization on every read. The bridge validates complete project-qualified relationships, then returns organization, department and member display names, availability and the maintenance time. Operational provenance and internal references stay on the service.

Members are shown as recorded in that project's directory. A directory may cover a subset of its people, and an available member record does not establish that an Agent is connected or executing work. The page provides member search, department filtering, name sorting, manual refresh and links to the existing personal connection controls. Each project is read separately, and same-name members retain separate rows. Relationship changes continue through the authorized Agent entries.

## Focused verification

The domain tests cover shape, bounds, reference validation, order-independent hashing and the hierarchy rules (cycles, the depth bound, sibling name uniqueness, and pre-hierarchy plans reading as all top level). PostgreSQL tests cover conditional writes, concurrent attempts, exact replay, recovery, revoked authority, project-qualified identities, the audit-summary hierarchy fields and rejected hierarchy faults leaving no row behind. Native MCP and HTTP tests exercise Agent delegation and the shared service entry points.

```bash
cargo test -p awr-team --lib directory --locked
cargo test -p awr-team-pg --features pg-tests --test pg_directory --locked -- --test-threads=1
cargo test -p awr-server --features pg-tests --test directory_mcp --locked -- --test-threads=1
```

The PostgreSQL fixtures use their configured loopback test server and create isolated process-owned databases.

The `directory_mcp` test also writes the verbatim `directory.read` replies of the real service entries (agent bearer, browser session, unconfigured, flat and hierarchical states) into the directory named by `AWR_DIRECTORY_SAMPLE_DIR` for evidence capture. Byte-identical copies are committed at `crates/awr-server/tests/samples/` and re-parsed on every run against the live replies, so the recorded shapes cannot drift from what the service produces. Verification against a deployed service remains with AWR-PPD-090.
