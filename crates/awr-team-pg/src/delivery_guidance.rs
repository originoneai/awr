//! Shared next-action wording for a repository integration that was refused because the target
//! moved.
//!
//! An admitted execution observes every workspace effect, imported baseline paths included, and
//! fails with a scope violation when an observed path lies outside its declared scope (see
//! `execution.rs`). A target that moved by files outside the work's own scope therefore cannot be
//! imported inside the next coding execution. The workspace instructions call that update "a
//! separate source-reviewed task performed before coding begins or after all earlier effects are
//! settled". Guidance that told the developer to update "in a new admitted execution" led a native
//! run into exactly that failure, so every site that names this next action builds it from the
//! text below: the integration inspection, the owner's work inbox and the refusal to complete a
//! work whose integration was rejected because the target moved.

/// What the responsible developer does after a changed target, without the surrounding sentence.
macro_rules! revision_steps {
    () => {
        "The responsible developer updates the workspace to the current target as a separate \
         baseline step before the next coding execution begins (an admitted execution observes \
         imported baseline paths and fails with a scope violation when they lie outside its \
         declared scope; if the project requires a source-reviewed baseline task for those paths, \
         the supervisor creates it first), then retests in a new admitted execution, publishes a \
         new candidate with its actual target precondition and obtains fresh independent review."
    };
}

/// Next action of a rejected integration whose target moved, as shown by the integration
/// inspection to every authorized reader.
pub(crate) const TARGET_CHANGED_INSPECT_ACTION: &str = concat!(
    "The repository target changed. Use work.prepare to inspect responsibility and blockers. ",
    revision_steps!(),
    " Supervisors coordinate that revision; do not retry or finalize the rejected version."
);

/// Next action of the `IntegrationNotConfirmed` refusal whose reason is `integration_target_changed`:
/// the same order, followed by what completes the work.
pub(crate) const TARGET_CHANGED_COMPLETION_ACTION: &str = concat!(
    revision_steps!(),
    " Then prepare that candidate's integration, wait until it is confirmed and finalize. Do not \
     finalize the rejected version."
);

/// Next action in the work inbox of the developer who owns the rejected work. Inbox guidance is
/// bounded (one item must stay under 1000 bytes), so this is the short form of
/// [`TARGET_CHANGED_INSPECT_ACTION`]; the inspection carries the full text.
pub(crate) const TARGET_CHANGED_OWN_REJECTION_ACTION: &str = "Inspect the rejection. If the target \
    changed, use work.prepare to resume your own session/claim, update the workspace to the \
    current target before the next coding execution begins (an admitted execution fails with a \
    scope violation on imported paths outside its scope), then retest in a new admitted execution \
    and request independent review of a new candidate. Otherwise resolve the recorded refusal \
    before retrying; old approval cannot authorize changed content.";

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [&str; 3] = [
        TARGET_CHANGED_INSPECT_ACTION,
        TARGET_CHANGED_OWN_REJECTION_ACTION,
        TARGET_CHANGED_COMPLETION_ACTION,
    ];

    #[test]
    fn every_text_puts_the_baseline_update_before_the_next_execution() {
        for text in ALL {
            let update = text
                .find("before the next coding execution begins")
                .expect("names the baseline update before coding begins");
            let execution = text
                .find("in a new admitted execution")
                .expect("then retests in a new admitted execution");
            assert!(
                update < execution,
                "update must precede the execution: {text}"
            );
            assert!(text.contains("scope violation"), "explains why: {text}");
        }
    }

    #[test]
    fn no_text_sends_the_update_into_an_admitted_execution() {
        for text in ALL {
            assert!(
                !text.contains("update against the current target"),
                "{text}"
            );
            assert!(
                !text.contains("revise in a new admitted execution"),
                "{text}"
            );
            assert!(!text.contains("rebuilds on the current target"), "{text}");
        }
    }

    #[test]
    fn the_wrapping_sentences_keep_their_stop_signs() {
        assert!(
            TARGET_CHANGED_INSPECT_ACTION.contains("do not retry or finalize the rejected version")
        );
        assert!(
            TARGET_CHANGED_OWN_REJECTION_ACTION
                .contains("old approval cannot authorize changed content")
        );
        assert!(TARGET_CHANGED_OWN_REJECTION_ACTION.contains("resume your own session/claim"));
        assert!(
            TARGET_CHANGED_COMPLETION_ACTION.contains("wait until it is confirmed and finalize")
        );
        assert!(
            TARGET_CHANGED_COMPLETION_ACTION.ends_with("Do not finalize the rejected version.")
        );
    }
}
