# Explicit member identity and simulated collaboration

A responsibility member is identified by `PersonId`. A model name, an Agent
session, or the number of physical operators does not establish that identity.
Each participant in a simulated collaboration uses a separate member anchor,
Agent actor, client, credential and explicit person-to-Agent binding. Several
simulated members may share the same physical controller.

Schema37 adds nullable `persons.member_identity` metadata. Existing rows remain
unspecified. This is provenance, never an authorization grant or evidence of a
human signature. Existing responsibility, lease and independent-review rules
continue to use their established identities and permissions.

## Initial registration

Use the existing owner `access agent-preview` / `access agent-apply` flow after
provisioning the member and Agent access identities. Add an optional field to the
initial Agent plan:

```json
{
  "member_identity": {
    "kind": "simulated_member",
    "controller_ref": "experiment-controller"
  }
}
```

The complete plan still requires the existing tenant, project and explicit
authorization fields. `controller_ref` is optional bounded, opaque experiment
attribution. It must contain no credential or private personal information;
sharing it creates no authority or review independence by itself.

For `simulated_member`, provision a separate non-human member anchor with actor
kind `agent`. The delegated execution Agent must have a different actor ID. A
human or system actor cannot be relabeled as a simulated member. For explicitly
declared `human` metadata, the existing member anchor must have kind `human`.
Omitting metadata preserves the legacy human-anchor provisioning contract while
leaving provenance unspecified.

Registration stores provenance, binding, authorization and audit receipts in one
transaction. Preview binds the current member state and exact plan. Changed
intent under the same request is refused; unchanged replay retains the original
receipt. Inspection exposes the stored `state.person.member_identity`, which is
null for unspecified provenance. The legacy `state.human_actor` response key is
retained for compatibility; its actual `kind` determines which anchor was used.

## Continuing work

Renewal and additional-scope plans accept the same optional metadata, but cannot
introduce or change provenance on an existing binding. Omitting the field uses
the already stored provenance; it never converts a simulated member into a
human. Credentials, scope grants, binding status and delegation lifetime are
rechecked normally. Identity metadata does not revive an expired run, bypass
revocation, grant a new project, or authorize replay of unknown effects.

Simulated review uses the separately authorized Agent-review contract and stays
at its existing trust level. Its receipts do not assert human approval or human
team acceptance. Two Agents bound to the same responsibility member remain the
same member for author/reviewer separation; different model labels do not make
self-review independent. Full business-scenario acceptance is separate from
these registration and regression checks.
