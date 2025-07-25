use botanix_activation_manager::{
    test_utils::{Db, ACTIVE_VERSION, UPGRADE_VERSION},
    ActivationManagerBuilder, OnProcessProposalDecision,
};
use botanix_storage::models::Vote;

#[test]
fn force_upgrade_checked() {
    let quorum = 100;
    let min_validator_count = 3;
    let target_height = 0;
    let our_vote = Some(Vote::Aye);

    let manager = ActivationManagerBuilder::new(Db::new(), ACTIVE_VERSION)
        .build_COMPLIANT_network_upgrade(
            UPGRADE_VERSION,
            quorum,
            min_validator_count,
            target_height,
            our_vote,
        );

    // Propose active version.
    let res = manager.on_prepare_proposal(100).unwrap();
    assert_eq!(res.version, ACTIVE_VERSION);

    // Reject upgraded version.
    let res = manager.on_process_proposal(100, UPGRADE_VERSION).unwrap();
    assert!(matches!(res, OnProcessProposalDecision::RejectBlock { version: UPGRADE_VERSION, .. }));

    // FORCE wrong upgrade version.
    manager.force_upgrade_checked(ACTIVE_VERSION);

    // Propose active version
    let res = manager.on_prepare_proposal(100).unwrap();
    assert_eq!(res.version, ACTIVE_VERSION);

    // Reject upgraded version.
    let res = manager.on_process_proposal(100, UPGRADE_VERSION).unwrap();
    assert!(matches!(res, OnProcessProposalDecision::RejectBlock { version: UPGRADE_VERSION, .. }));

    // FORCE tracked upgrade version; manager will immediately fast-forward to the upgrade.
    manager.force_upgrade_checked(UPGRADE_VERSION);

    // Propose upgraded version!
    let res = manager.on_prepare_proposal(100).unwrap();
    assert_eq!(res.version, UPGRADE_VERSION);

    // Back upgraded version!
    let res = manager.on_process_proposal(100, UPGRADE_VERSION).unwrap();
    assert!(matches!(res, OnProcessProposalDecision::Process { version: UPGRADE_VERSION, .. }));
}
