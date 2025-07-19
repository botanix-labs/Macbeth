//! A simple in-memory vote tracker used by the ActivationManager which only
//! tracks the last vote.
use std::{
    collections::HashMap,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vote_watcher_basic_properties() {
        let watcher = VoteWatcher::default();

        let alice = Address::from_slice(&[0; 20]);
        let bob = Address::from_slice(&[1; 20]);
        let eve = Address::from_slice(&[2; 20]);

        let min_val_count = 10;
        let botanix_height = 500;

        // Check basic init properties.
        //
        let ayes = watcher.get_upgrading_approval_rate_ayes(min_val_count).unwrap();
        assert_eq!(ayes, (0, min_val_count));

        let compliant = watcher.get_upgrading_approval_rate_compliance(min_val_count).unwrap();
        assert_eq!(compliant, (0, min_val_count));

        let removed = watcher.remove_upgrading_votes(botanix_height).unwrap();
        assert_eq!(removed, 0);

        // Track votes.
        //
        watcher.update_upgrading_vote(alice, Vote::Aye, true, botanix_height).unwrap();
        watcher.update_upgrading_vote(bob, Vote::Aye, false, botanix_height).unwrap();
        watcher.update_upgrading_vote(eve, Vote::Nay, false, botanix_height).unwrap();

        let ayes = watcher.get_upgrading_approval_rate_ayes(min_val_count).unwrap();
        assert_eq!(ayes, (20, min_val_count)); // 20%

        let compliant = watcher.get_upgrading_approval_rate_compliance(min_val_count).unwrap();
        assert_eq!(compliant, (10, min_val_count)); // 10%

        // Eve votes again
        //
        watcher.update_upgrading_vote(eve, Vote::Aye, true, botanix_height).unwrap();

        let ayes = watcher.get_upgrading_approval_rate_ayes(min_val_count).unwrap();
        assert_eq!(ayes, (30, min_val_count)); // 30%

        let compliant = watcher.get_upgrading_approval_rate_compliance(min_val_count).unwrap();
        assert_eq!(compliant, (20, min_val_count)); // 20%

        // Remove votes
        //
        let removed = watcher.remove_upgrading_votes(botanix_height).unwrap();
        assert_eq!(removed, 0);

        let removed = watcher.remove_upgrading_votes(botanix_height + 1).unwrap();
        assert_eq!(removed, 3); // alice, bob and eve

        // Removed; all is gone
        //
        let ayes = watcher.get_upgrading_approval_rate_ayes(min_val_count).unwrap();
        assert_eq!(ayes, (0, min_val_count));

        let compliant = watcher.get_upgrading_approval_rate_compliance(min_val_count).unwrap();
        assert_eq!(compliant, (0, min_val_count));

        let removed = watcher.remove_upgrading_votes(botanix_height).unwrap();
        assert_eq!(removed, 0);
    }
}
