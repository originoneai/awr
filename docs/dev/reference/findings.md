# Findings

A finding says something is wrong with a work. For a source that opts in to
[plan admission](admission.md), whether a finding sends the work back is not a matter of how loudly
it is put: its **severity is derived** from what it is measured against, which are the acceptance
tiers adopted for the work and the hard rules of the project, and from what it shows. Only
**blocking** findings return the work. Everything else is kept as a **deferred record**, linked to
the work and to what it was measured against, which blocks nothing. No migration, table, state or
task type is added.

## Recording a finding

`awr work finding --input finding.json` (or the MCP tool `awr_work_finding`) records one
`work.finding` event. `action` is `record` or `resolve`; a `finding_key` names the finding within
the work and branch for good.

Recording needs the review `round` (counted from 1) and a `summary`, and may name what the finding
is measured against and what it shows:

| Field | Meaning |
| --- | --- |
| `criterion` | an acceptance criterion of the work, word for word; its tier is the one adopted for it (the work's own `tier`, or `later` when every specification item bound to exactly that criterion is future) |
| `rule` | the key of a rule of the project; it counts when its severity is `hard` |
| `counterexample` | `given`, `when` and `then`: what to do so that someone else can see the finding for themselves |
| `risk` | a `security` or `data_loss` risk with the reasoning behind it |

## How the severity is derived

| The finding | Severity |
| --- | --- |
| states a security or data-loss risk with its reasoning | **blocking**, whatever the tier of the criterion |
| breaks a required criterion or a hard rule and shows it with a counterexample | **blocking** |
| names a required criterion or a hard rule but has no reproducible counterexample | non-blocking |
| names a criterion that is later-tier, a rule that is not hard, or nothing the project adopted | non-blocking: a matter of taste or style |

A second-round style finding is therefore deferred. The severity and the reason for it are kept with
the event, so a later change of tier does not rewrite what a review found. A criterion has to be one
of the work's acceptance criteria and a rule has to exist; otherwise the request is refused.

## What blocking and deferred do

An open blocking finding holds **the completion of that work**: `work complete` is refused with
`DependencyBlocked`, naming the findings, until each is resolved (`action: resolve` with how it was
dealt with). Deferred records are listed under `findings` in `work show`, `work prepare` and
`awr_work_get` with `disposition: deferred`; they never hold completion, and resolving one closes
the record. Nothing else changes: other work, claims and execution are untouched by a finding, and
the verification of acceptance and evidence stays as it was.

A blocking finding against work that is not completed, and a reopening of completed work, are both
returns of the work, and they count as rework cycles against its
[budget](reassessment.md#budgets). Several blocking findings of one review round are one return,
and a blocking finding against completed work is answered by the reopening that follows, so one
return is never counted twice. Deferred records count for nothing.

## History

Events are append-only: a resolved finding stays listed, with who raised it, who dealt with it and
how. Requests replay by key, and a closed finding cannot be closed again.

The program derives severity from the record and enforces what it implies. Whether the
counterexample is right, whether a risk is real and whether the finding is fair stay judgments of
the reviewer.
