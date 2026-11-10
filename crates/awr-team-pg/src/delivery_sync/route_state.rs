//! Registration and task-entry facts, never execution or worker-liveness admission.
use super::*;
use serde_json::{Value, json};
use tokio_postgres::Transaction;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum TaskRouteState {
    Missing,
    Registered,
    RequiresAdministration,
    Entered,
}

impl TaskRouteState {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Missing => "missing",
            Self::Registered => "registered",
            Self::RequiresAdministration => "requires_administration",
            Self::Entered => "entered",
        }
    }

    pub(crate) fn guidance(self, templates_available: bool, continuation: bool) -> Value {
        let (condition, action) = match self {
            Self::Registered => (
                "This task already has a current enabled delivery registration",
                json!({"op":"work.prepare","note":"Keep the existing mapping and follow current dispatch, claim or development guidance; do not reuse another route."}),
            ),
            Self::RequiresAdministration => (
                "This task has an existing mapping that is not current and enabled",
                json!({"op":"delivery.neutral.inspect","note":"Use the established operator administration path to repair the existing mapping; route reuse cannot replace it."}),
            ),
            Self::Entered => (
                "This unmapped task has unsettled execution, delivery history or a closed outcome",
                json!({"op":"work.prepare","note":"Inspect the original work and effects before changing delivery registration. Settle active execution normally; unresolved or historical delivery effects retain their established recovery path. Do not reset work or replay effects to obtain a route."}),
            ),
            Self::Missing if templates_available => (
                "This task has no delivery mapping or unsettled execution or prior delivery effects",
                json!({"op":"delivery.route.reuse","role":"supervisor","note":"Select one listed template and its exact versions with a fresh work.prepare header."}),
            ),
            Self::Missing if continuation => (
                "This continuation page contains no current template",
                json!({"op":"delivery.routes","note":"Restart discovery without a cursor; an empty continuation page does not prove there are no authorized templates."}),
            ),
            Self::Missing => (
                "No task mapping or authorized route template is registered",
                json!({"kind":"request_operator","role":"project_admin","note":"Repository delivery needs operator configuration; report-only work needs no route."}),
            ),
        };
        json!({"when":condition,"because":"Current task and connector registrations; no permission or worker liveness is inferred",
            "action":action,"recheck_on":"Task, mapping, template or authority changes; inspect delivery.neutral.outcome after an uncertain response"})
    }
}

impl DeliverySyncStore {
    /// Call only after authenticating and resolving the exact current work binding.
    pub(crate) async fn task_route_state_in_tx(
        tx: &Transaction<'_>,
        tenant: &str,
        project: &str,
        work: &str,
        workstream: &str,
        epoch: &str,
    ) -> PgResult<TaskRouteState> {
        let row = tx.query_one("SELECT
            EXISTS(SELECT 1 FROM awr_team.delivery_connectors
                WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3),
            EXISTS(SELECT 1 FROM awr_team.delivery_connectors c
                JOIN awr_team.actors a ON a.tenant_id=c.tenant_id AND a.id=c.principal_actor_id
                WHERE c.tenant_id=$1 AND c.project_id=$2 AND c.work_id=$3 AND c.workstream_id=$4
                    AND c.enabled AND c.coordinator_epoch=$5 AND a.status='active'),
            EXISTS(SELECT 1 FROM awr_team.executions WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3
                AND (state NOT IN ('succeeded','failed','cancelled')
                    OR (settlement_policy_json IS NOT NULL AND NOT workspace_effects_settled)
                    OR admission_mode='reference_write_v1')) OR
            EXISTS(SELECT 1 FROM awr_team.resource_reservations
                WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3 AND state='unknown') OR
            EXISTS(SELECT 1 FROM awr_team.delivery_inspections i
                JOIN awr_team.delivery_candidates c USING(tenant_id,project_id,binding_digest)
                WHERE i.tenant_id=$1 AND i.project_id=$2 AND c.work_id=$3) OR
            EXISTS(SELECT 1 FROM awr_team.delivery_integration_intents
                WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3) OR
            EXISTS(SELECT 1 FROM awr_team.delivery_source_publications
                WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3) OR
            EXISTS(SELECT 1 FROM awr_team.completion_receipts
                WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3) OR
            EXISTS(SELECT 1 FROM awr_team.work_runtime WHERE tenant_id=$1 AND project_id=$2 AND work_id=$3
                AND (recovery_blocked OR state IN ('completed','cancelled','archived') OR selected_completion_id IS NOT NULL))",
            &[&tenant,&project,&work,&workstream,&epoch]).await?;
        if row.get::<_, bool>(1) {
            return Ok(TaskRouteState::Registered);
        }
        if row.get::<_, bool>(0) {
            return Ok(TaskRouteState::RequiresAdministration);
        }
        Ok(if row.get::<_, bool>(2) {
            TaskRouteState::Entered
        } else {
            TaskRouteState::Missing
        })
    }
}
