//! Closing a session after the work it held is completed or cancelled: one bounded instruction in
//! the response, and, when the caller asks for it, the end of the session in the same request.
//! Ending a session never completes work, and completing work never ends a session unasked.
use awr_core::*;
use awr_store::Store;
use serde::Serialize;

/// One instruction (condition, basis, next step, when to look again) for what to do now that the
/// work is finished. It is told only while the session holds no other active claim, and it adds
/// well under 60 tokens to a response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CloseOut {
    pub condition: &'static str,
    pub basis: &'static str,
    pub next: &'static str,
    pub recheck: &'static str,
}

/// What ending the session did after the completion that came first in the same request.
#[derive(Debug, Clone, Serialize)]
pub struct SessionEndReceipt {
    /// `ended`, or `failed` when the completion stands and only the end did not happen.
    pub status: &'static str,
    pub session_id: Id,
    /// The `work.completed` event of the same request.
    pub completion_event_id: Id,
    pub event_id: Option<Id>,
    pub closed_claim_ids: Vec<Id>,
    /// What to do when the end failed; the completion is not undone.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next: Option<&'static str>,
}

const ENDED: &str = "the work is completed and its claim is released";
const CANCELLED: &str = "the work is cancelled and its claim is released";

/// The instruction after a completion or a cancellation, none while the session still holds
/// another active claim or is not active any more.
pub(crate) fn close_out(
    store: &Store,
    project: Id,
    session: Id,
    action: WorkAction,
) -> Option<CloseOut> {
    let basis = match action {
        WorkAction::Complete => ENDED,
        WorkAction::Cancel => CANCELLED,
        _ => return None,
    };
    if store.session(project, session).ok()?.status != "active" {
        return None;
    }
    let now = now_millis().ok()?;
    let claims = store.session_claims(project, session).ok()?;
    if claims.iter().any(|claim| claim.active_at(now)) {
        return None;
    }
    Some(CloseOut {
        condition: "this session holds no other active claim",
        basis,
        next: "save a checkpoint, then run session end --outcome ended",
        recheck: "when this session claims other work",
    })
}

/// Refuses to end a session that still holds claims on other work than the one being completed.
pub(crate) fn require_no_other_claims(
    store: &Store,
    project: Id,
    session: Id,
    work: Id,
) -> Result<()> {
    if store.session(project, session)?.status != "active" {
        return Err(Error::InvalidInput(
            "the session is not active, so it cannot be ended with the completion".into(),
        ));
    }
    let now = now_millis()?;
    let mut others = Vec::new();
    for claim in store.session_claims(project, session)? {
        if claim.work_item_id != work && claim.active_at(now) {
            others.push(
                store
                    .work_item_by_id(project, claim.work_item_id)?
                    .item
                    .meta
                    .external_key,
            );
        }
    }
    if others.is_empty() {
        return Ok(());
    }
    Err(Error::ClaimConflict(format!(
        "the session still holds claims on other work ({}): finish or release them, or complete without ending the session",
        others.join(", ")
    )))
}

/// Ends the session after a completion, with the live revision: nothing but this end depends on it,
/// and a concurrent writer must not turn a durable completion into a half-finished request.
pub(crate) fn end_after_completion(
    store: &mut Store,
    project: Id,
    session: Id,
    completion: Id,
) -> SessionEndReceipt {
    let mut last = None;
    for _ in 0..3 {
        let result = store.project(project).and_then(|current| {
            store.end_session_after(
                project,
                current.project_revision,
                session,
                SessionOutcome::Ended,
                Some(completion),
            )
        });
        match result {
            Ok((_, event)) => {
                let closed = event.payload["closed_claim_ids"]
                    .as_array()
                    .map(|ids| {
                        ids.iter()
                            .filter_map(|id| id.as_str()?.parse().ok())
                            .collect()
                    })
                    .unwrap_or_default();
                return SessionEndReceipt {
                    status: "ended",
                    session_id: session,
                    completion_event_id: completion,
                    event_id: Some(event.id),
                    closed_claim_ids: closed,
                    next: None,
                };
            }
            Err(error @ Error::RevisionConflict { .. }) => last = Some(error),
            Err(error) => {
                last = Some(error);
                break;
            }
        }
    }
    let _ = last;
    SessionEndReceipt {
        status: "failed",
        session_id: session,
        completion_event_id: completion,
        event_id: None,
        closed_claim_ids: vec![],
        next: Some("the completion stands; run session end --outcome ended to end the session"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tokens(close_out: &CloseOut) -> usize {
        awr_context::token_count(&serde_json::to_string(close_out).unwrap())
    }

    #[test]
    fn the_instruction_adds_well_under_sixty_tokens_for_either_action() {
        for basis in [ENDED, CANCELLED] {
            let instruction = CloseOut {
                condition: "this session holds no other active claim",
                basis,
                next: "save a checkpoint, then run session end --outcome ended",
                recheck: "when this session claims other work",
            };
            let used = tokens(&instruction);
            assert!(used <= 60, "{used} tokens: {instruction:?}");
        }
    }
}
