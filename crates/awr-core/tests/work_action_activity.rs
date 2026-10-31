//! Which work actions are work done under a claim; a policy that renews on activity renews on these and on no other.
use awr_core::WorkAction;

#[test]
fn only_progress_block_and_unblock_are_activity() {
    let all = [
        WorkAction::Progress,
        WorkAction::Block,
        WorkAction::Unblock,
        WorkAction::Cancel,
        WorkAction::Reopen,
        WorkAction::Complete,
    ];
    let activity: Vec<_> = all.into_iter().filter(|a| a.is_activity()).collect();
    assert_eq!(
        activity,
        [WorkAction::Progress, WorkAction::Block, WorkAction::Unblock]
    );
}
