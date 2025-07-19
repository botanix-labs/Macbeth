use super::*;
use reth_provider::ProviderResult;
use std::{collections::HashMap, sync::Arc};
use utils::Address;

mod accept_lagging_validator;
mod reject_ignored_upgrade;
mod reject_mismatched_vote;
mod reject_outdated_or_unsupported_version;
mod utils;
mod vote_expiration;
mod vote_majority_nay_compliant_and_reject;
mod vote_nay_and_accept;
mod vote_nay_and_reject;
mod wait_for_compliance;

/// In-Memory database that integrates the `ActivationManagerReaderWriter` trait.
#[derive(Clone)]
struct Db {
    votes: Arc<RwLock<HashMap<Address, VoteEntry>>>,
}

impl Db {
    fn new() -> Self {
        Db { votes: Arc::new(RwLock::new(HashMap::new())) }
    }
}

struct VoteEntry {
    vote: Vote,
    is_compliant: bool,
    botanix_height: u64,
}

impl ActivationManagerReaderWriter<utils::Address> for Db {
    fn update_upgrading_vote(
        &self,
        auth: Address,
        vote: Vote,
        is_compliant: bool,
        botanix_height: u64,
    ) -> ProviderResult<()> {
        let mut votes = self.votes.write().unwrap();

        #[rustfmt::skip]
        votes
            .entry(auth)
            .and_modify(|e| {
                e.vote = vote;
                e.is_compliant = is_compliant;
                e.botanix_height = botanix_height;
            })
            .or_insert(VoteEntry {
                vote,
                is_compliant,
                botanix_height,
            });

        Ok(())
    }

    fn get_aye_votes(&self) -> ProviderResult<(usize, usize)> {
        let votes = self.votes.read().unwrap();
        let ayes = votes.iter().filter(|(_, e)| e.vote == Vote::Aye).count();
        Ok((ayes, votes.len()))
    }

    fn get_nay_votes(&self) -> ProviderResult<(usize, usize)> {
        let votes = self.votes.read().unwrap();
        let nays = votes.iter().filter(|(_, e)| e.vote == Vote::Nay).count();
        Ok((nays, votes.len()))
    }

    fn get_abstained_votes(&self) -> ProviderResult<(usize, usize)> {
        let votes = self.votes.read().unwrap();
        let abstains = votes.iter().filter(|(_, e)| e.vote == Vote::Absent).count();
        Ok((abstains, votes.len()))
    }

    fn get_compliance_count(&self) -> ProviderResult<(usize, usize)> {
        let votes = self.votes.read().unwrap();
        let compliant = votes.iter().filter(|(_, e)| e.is_compliant).count();
        Ok((compliant, votes.len()))
    }

    fn get_upgrading_approval_rate_ayes(
        &self,
        min_validator_count: usize,
    ) -> ProviderResult<(usize, usize)> {
        let votes = self.votes.read().unwrap();

        let total = votes.len().max(min_validator_count);
        let votes_received = votes.iter().filter(|(_, e)| e.vote == Vote::Aye).count();

        // Calculate percentage (0-100) of votes received
        let quorum = (votes_received * 100).div_ceil(total);

        Ok((quorum, total))
    }

    fn get_upgrading_approval_rate_compliance(
        &self,
        min_validator_count: usize,
    ) -> ProviderResult<(usize, usize)> {
        let votes = self.votes.read().unwrap();

        let total = votes.len().max(min_validator_count);
        let votes_received = votes.iter().filter(|(_, e)| e.is_compliant).count();

        // Calculate percentage (0-100) of votes received
        let quorum = (votes_received * 100).div_ceil(total);

        Ok((quorum, total))
    }

    fn remove_upgrading_votes(&self, botanix_height: u64) -> ProviderResult<usize> {
        let mut votes = self.votes.write().unwrap();

        let gross_total = votes.len();
        votes.retain(|_, e| e.botanix_height >= botanix_height);

        Ok(gross_total - votes.len())
    }
}

#[test]
fn activation_manager_basic_db_interface() {
    let db = Db::new();

    // Generate addresses
    let alice = b"alice".to_vec();
    let bob = b"bob".to_vec();
    let eve = b"eve".to_vec();

    let min_validator_count = 3;

    // Alice votes Aye, is not compliant
    db.update_upgrading_vote(alice, Vote::Aye, false, 0).unwrap();
    let res = db.get_upgrading_approval_rate_ayes(min_validator_count).unwrap();
    assert_eq!(res, (34, 3));
    let res = db.get_upgrading_approval_rate_compliance(min_validator_count).unwrap();
    assert_eq!(res, (0, 3));

    // Bob votes Nay, is not compliant
    db.update_upgrading_vote(bob, Vote::Nay, false, 0).unwrap();
    let res = db.get_upgrading_approval_rate_ayes(min_validator_count).unwrap();
    assert_eq!(res, (34, 3));
    let res = db.get_upgrading_approval_rate_compliance(min_validator_count).unwrap();
    assert_eq!(res, (0, 3));

    // Eve votes Aye, IS compliant
    db.update_upgrading_vote(eve, Vote::Aye, true, 0).unwrap();
    let res = db.get_upgrading_approval_rate_ayes(min_validator_count).unwrap();
    assert_eq!(res, (67, 3));
    let res = db.get_upgrading_approval_rate_compliance(min_validator_count).unwrap();
    assert_eq!(res, (34, 3));

    // Adjust minimum voter count (ABOVE total votes)
    let min_validator_count = 5;
    let res = db.get_upgrading_approval_rate_ayes(min_validator_count).unwrap();
    assert_eq!(res, (40, 5));
    let res = db.get_upgrading_approval_rate_compliance(min_validator_count).unwrap();
    assert_eq!(res, (20, 5));

    // Adjust minimum voter count (BELOW total votes)
    let min_validator_count = 1;
    let res = db.get_upgrading_approval_rate_ayes(min_validator_count).unwrap();
    assert_eq!(res, (67, 3));
    let res = db.get_upgrading_approval_rate_compliance(min_validator_count).unwrap();
    assert_eq!(res, (34, 3));

    // Remove votes
    let res = db.remove_upgrading_votes(1).unwrap();
    assert_eq!(res, 3);
    let res = db.get_upgrading_approval_rate_ayes(min_validator_count).unwrap();
    assert_eq!(res, (0, 1));
    let res = db.get_upgrading_approval_rate_compliance(min_validator_count).unwrap();
    assert_eq!(res, (0, 1));
}
