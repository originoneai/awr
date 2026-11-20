# Activity categories

The Activity and audit page uses `OPERATION_VOCABULARY` and `tools/observer/src/i18n/activity-categories.ts`.

The seven filters are Delivery, Review, Decision, Assignment, Report, Breakdown and Update. **All only** means that an operation is visible under All but under none of the seven filters. An operation absent from the registry is also All only and receives the neutral fallback operation label.

This is display classification, not a new service fact and not a permission boundary. The registry contains the request operations and committed event names exposed by the current service, including the operations present in the captured live replies. `activity-categories.test.ts` checks every registry row against the runtime filter, rejects duplicate operation names, requires bilingual labels and checks this table.

The service's activity read selects request-audit actions and committed event types in `crates/awr-team-pg/src/workstream_read/activity.rs`; operation declarations and committed events originate in `crates/awr-server/src/service/mcp.rs` and `crates/awr-team-pg/src/workstream_command.rs` with its command modules. When the service adds an operation that is absent from the vocabulary, the page keeps it under All with the generic product label. Add a deliberate bilingual vocabulary and registry entry when its product meaning is known.

| Operation | Category |
| --- | --- |
| `access.changed` | All only |
| `admission.decide` | Decision |
| `agent.authorization.issued` | All only |
| `artifact.content` | All only |
| `artifact.segment` | All only |
| `audit.count` | All only |
| `audit.development` | All only |
| `audit.export` | All only |
| `audit.history` | All only |
| `audit.requests` | All only |
| `awr_team_input_contract` | All only |
| `capabilities` | All only |
| `claim.acquire` | Assignment |
| `claim.expired` | Assignment |
| `claim.inspect` | Assignment |
| `claim.release` | Assignment |
| `claim.renew` | Assignment |
| `command.inspect` | All only |
| `completion.inspect` | Delivery |
| `decision.cancel` | Decision |
| `decision.request` | Decision |
| `decision.respond` | Decision |
| `decisions.list` | Decision |
| `delivery.adopt` | Delivery |
| `delivery.candidate.select` | Delivery |
| `delivery.connector.configure` | Delivery |
| `delivery.evidence.submit` | Delivery |
| `delivery.export.publish` | Delivery |
| `delivery.export.revoke` | Delivery |
| `delivery.exports` | Delivery |
| `delivery.facts.ingest` | Delivery |
| `delivery.finalize` | Delivery |
| `delivery.inspect` | Delivery |
| `delivery.inspection.reserve` | Delivery |
| `delivery.integration.confirm` | Delivery |
| `delivery.integration.dispatch` | Delivery |
| `delivery.integration.inspect` | Delivery |
| `delivery.integration.inspection.reserve` | Delivery |
| `delivery.integration.lease` | Delivery |
| `delivery.integration.prepare` | Delivery |
| `delivery.integration.reject_prepared` | Delivery |
| `delivery.neutral.inspect` | Delivery |
| `delivery.neutral.outcome` | Delivery |
| `delivery.observe_pr` | Delivery |
| `delivery.pump.claim` | Delivery |
| `delivery.pump.refresh` | Delivery |
| `delivery.register_pr` | Delivery |
| `delivery.source.abandon` | Delivery |
| `delivery.source.confirm` | Delivery |
| `delivery.source.prepare` | Delivery |
| `delivery.source.renew` | Delivery |
| `delivery.source.status` | Delivery |
| `delivery.source.write` | Delivery |
| `delivery.submission.describe` | Delivery |
| `delivery.submit_and_request_review` | Delivery |
| `documents.list` | All only |
| `events.list` | All only |
| `evidence.inspect` | Delivery |
| `evidence.recorded` | Delivery |
| `evidence.submit` | Delivery |
| `execution.attest` | Report |
| `execution.cancel` | All only |
| `execution.inspect` | All only |
| `execution.prepare` | All only |
| `execution.reconcile` | All only |
| `execution.report` | Report |
| `execution.start` | All only |
| `goal.inspect` | All only |
| `goals.list` | All only |
| `handoff.accept` | Assignment |
| `handoff.cancel` | Assignment |
| `handoff.inspect` | Assignment |
| `handoff.propose` | Assignment |
| `handoff.reject` | Assignment |
| `handoff.timeout` | Assignment |
| `lesson.inspect` | All only |
| `lessons.list` | All only |
| `planning.approve` | Breakdown |
| `planning.candidates` | Breakdown |
| `planning.edit_draft` | Breakdown |
| `planning.outcome` | Breakdown |
| `planning.propose` | Breakdown |
| `planning.publish` | Breakdown |
| `query` | All only |
| `review.accept` | Review |
| `review.decide` | Review |
| `review.decided` | Review |
| `review.inspect` | Review |
| `review.open` | Review |
| `review.opened` | Review |
| `review.return` | Review |
| `review.rounds` | Review |
| `session.checkpoint` | Assignment |
| `session.end` | All only |
| `session.inspect` | All only |
| `session.start` | All only |
| `source.activated` | All only |
| `source.content` | All only |
| `sources.inspect` | All only |
| `task.accept_assignment` | Assignment |
| `task.assign` | Assignment |
| `task.assignees` | Assignment |
| `task.claim_available` | Assignment |
| `update.inspect` | Update |
| `update.record` | Update |
| `update.recorded` | Update |
| `updates.list` | Update |
| `work.admission` | All only |
| `work.complete` | Delivery |
| `work.completed` | Delivery |
| `work.graph` | All only |
| `work.inbox` | All only |
| `work.list` | All only |
| `work.next` | All only |
| `work.observe` | All only |
| `work.prepare` | All only |
| `work.recovery` | All only |
| `work.rework` | Breakdown |
| `work.search` | All only |
| `work.snapshot` | All only |
| `workstreams.list` | All only |
