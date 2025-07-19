//! A simple in-memory vote tracker used by the ActivationManager which only
//! tracks the last vote.
use std::{
    collections::{hash_map::Entry, HashMap},
    sync::{Arc, RwLock},
};

use reth_db::models::Vote;
use reth_primitives::Address;
use reth_provider::{ActivationManagerReaderWriter, ProviderResult};

/// A simple in-memory vote tracker used by the ActivationManager which only
/// tracks the last vote.
#[derive(Debug, Clone, Default)]
pub struct VoteWatcher {
    votes: Arc<RwLock<HashMap<Address, VoteEntry>>>,
}

#[derive(Debug)]
struct VoteEntry {
    vote: Vote,
    is_compliant: bool,
    botanix_height: u64,
}

// NOTE: This implementation was essentially copied 1-to-1 from the
// ActivationManager unit tests.
impl ActivationManagerReaderWriter<Address> for VoteWatcher {
    fn update_upgrading_vote(
        &self,
        auth: Address,
        vote: Vote,
        is_compliant: bool,
        botanix_height: u64,
    ) -> ProviderResult<()> {
        let mut votes = self.votes.write().unwrap();

        match votes.entry(auth) {
            Entry::Occupied(mut e) => {
                // If the validator has previously voted, their vote will be
                // updated to the new values if and only if the botanix height
                // is greater than the existing botanix height.
                if e.get().botanix_height >= botanix_height {
                    return Ok(())
                }

                let e = e.get_mut();
                e.vote = vote;
                e.is_compliant = is_compliant;
                e.botanix_height = botanix_height;
            }
            Entry::Vacant(v) => {
                let _ = v.insert(VoteEntry { vote, is_compliant, botanix_height });
            }
        }

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

#[cfg(test)]
mod tests {
    use super::*;
    use reth_provider::activation_manager_conformance_tests;

    #[test]
    fn vote_watcher_db_conformance() {
        let alice = Address::from_slice(&[0; 20]);
        let bob = Address::from_slice(&[1; 20]);
        let eve = Address::from_slice(&[2; 20]);

        activation_manager_conformance_tests::assert_threshold_rates(
            alice,
            bob,
            eve,
            VoteWatcher::default(),
        );

        activation_manager_conformance_tests::assert_polling(
            alice,
            bob,
            eve,
            VoteWatcher::default(),
        );
    }
}
