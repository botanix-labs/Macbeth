use crate::test_utils::fixtures::Address;
use botanix_storage::{models::Vote, ActivationManagerReaderWriter};
use reth_storage_errors::provider::ProviderResult;
use std::{
    collections::{hash_map::Entry, HashMap},
    sync::{Arc, RwLock},
};

/// In-Memory database that integrates the `ActivationManagerReaderWriter` trait.
#[derive(Clone)]
pub struct Db {
    votes: Arc<RwLock<HashMap<Address, VoteEntry>>>,
}

impl Db {
    pub fn new() -> Self {
        Db { votes: Arc::new(RwLock::new(HashMap::new())) }
    }
}

pub struct VoteEntry {
    vote: Vote,
    is_compliant: bool,
    botanix_height: u64,
}

impl ActivationManagerReaderWriter<Address> for Db {
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
        let abstains = votes.iter().filter(|(_, e)| e.vote == Vote::Abstain).count();
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
    use crate::test_utils::{assert_polling, assert_threshold_rates};

    #[test]
    fn unit_test_db_conformance() {
        let alice = b"alice".to_vec();
        let bob = b"bob".to_vec();
        let eve = b"eve".to_vec();

        #[rustfmt::skip]
        assert_threshold_rates(
            alice.clone(),
            bob.clone(),
            eve.clone(),
            Db::new(),
        );

        #[rustfmt::skip]
        assert_polling(
            alice,
            bob,
            eve,
            Db::new()
        );
    }
}
