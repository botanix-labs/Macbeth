use std::collections::HashMap;

use crate::utils::create_authority_sighash;
use botanix_lib::extra_data_header::{ExtraDataHeader, ExtraDataHeaderDeserialzeError};
use reth_primitives::{
    constants::eip225::{NONCE_AUTH, NONCE_DROP},
    Header,
};

/// Represents a vote to add or remove an authority.
#[derive(Debug, Clone, PartialEq, Copy)]
pub enum Vote {
    Add,
    Remove,
}

/// Tries to convert a u64 value to a Vote.
/// * `value` - The u64 value to convert.
///
/// # Errors
///
/// Returns an error if the u64 value is not valid for an EIP225 Authority Vote.
impl TryFrom<u64> for Vote {
    type Error = &'static str;

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        match value {
            NONCE_AUTH => Ok(Vote::Add),
            NONCE_DROP => Ok(Vote::Remove),
            _ => Err("Invalid u64 value for EIP225 Authority Vote"),
        }
    }
}

/// A collection of votes from federation member to add/remove a particular authority
#[derive(Debug, Clone, PartialEq)]
pub struct AuthorityVote {
    /// Authority to add/remove
    pub authority: secp256k1::PublicKey,
    /// Votes for this authority
    pub votes: HashMap<secp256k1::PublicKey, Vote>,
}

impl AuthorityVote {
    pub fn add_vote(&mut self, authority_voting: &secp256k1::PublicKey, vote: Vote) {
        // Check vote from this authority does not already exist
        if self.votes.contains_key(&authority_voting) {
            return
        }
        self.votes.insert(*authority_voting, vote);
    }

    pub fn contains(&self, authority: secp256k1::PublicKey) -> bool {
        self.votes.contains_key(&authority)
    }

    pub fn contains(&self, authority: secp256k1::PublicKey) -> bool {
        self.votes.contains_key(&authority)
    }
}

/// Utility struct to keep track of votes for a epoch
#[derive(Debug, Clone, Default)]
pub struct AuthorityVoteCollection {
    /// Votes for this epoch
    pub votes: Vec<AuthorityVote>,
    /// Starting of epoch
    pub epoch_start_block_height: u64,
}

impl AuthorityVoteCollection {
    pub fn vote_for(
        &mut self,
        authority_voting: &secp256k1::PublicKey,
        vote: &Vote,
        authority_vote_for: &secp256k1::PublicKey,
    ) {
        if let Some(authVote) = self.votes.iter_mut().find(|k| k.authority == *authority_vote_for) {
            authVote.add_vote(&authority_voting.clone(), vote.clone());
        } else {
            let mut votes = HashMap::new();
            votes.insert(*authority_voting, vote.clone());
            self.votes.push(AuthorityVote { authority: *authority_vote_for, votes });
        }
    }
}

#[derive(Debug)]
pub enum GetVotesError {
    FailedToDeserializeBlockHeaderExtraData(ExtraDataHeaderDeserialzeError),
    FailedToRecoverAuthority(secp256k1::Error),
    FailedToParseNonceVote,
}

/// Given a range of block headers we want a utility function that will return a list of votes
pub(crate) fn get_vote_results(headers: Vec<Header>) -> Result<Vec<AuthorityVote>, GetVotesError> {
    // Structure to keep track of all votes that occured in this block range
    let mut auth_vote: Vec<AuthorityVote> = Vec::new();

    for header in headers {
        if header.is_empty() {
            continue
        }
        // Check if there is a authority being voted on in the extra data
        let extra_data_header = ExtraDataHeader::deserialize(header.extra_data.0.to_vec())
            .map_err(|e| GetVotesError::FailedToDeserializeBlockHeaderExtraData(e))?;

        if extra_data_header.authority_vote.is_none() {
            continue
        }

        // If there is no signature, we can't verify who casted the vote
        // This would be a invalid block anyways
        if extra_data_header.authority_signature.is_none() {
            continue
        }

        // Check if there is a valid vote in the nonce field
        if header.nonce != NONCE_AUTH && header.nonce != NONCE_DROP {
            continue
        }

        let authority_to_vote_on = extra_data_header.authority_vote.expect("valid authority vote");
        // Need to recover the authority that signed the block from the signature
        // TODO(armins) remove unwrap
        let sig_hash = secp256k1::Message::from_slice(
            create_authority_sighash(&mut header.clone(), &extra_data_header).as_slice(),
        )
        .unwrap();

        let authority_that_votes = extra_data_header
            .authority_signature
            .expect("valid signature")
            .recover(&sig_hash)
            .map_err(|e| GetVotesError::FailedToRecoverAuthority(e))?;
        // Already keeping track of this authority
        if let Some(current_votes) =
            auth_vote.iter_mut().find(|k| k.authority == authority_to_vote_on)
        {
            // Check if the authority that signed block currently has a vote for this authority
            current_votes.add_vote(
                &authority_that_votes,
                header.nonce.try_into().map_err(|_e| GetVotesError::FailedToParseNonceVote)?,
            );
        } else {
            let mut votes = HashMap::new();
            votes.insert(
                authority_that_votes,
                header.nonce.try_into().map_err(|_e| GetVotesError::FailedToParseNonceVote)?,
            );
            auth_vote.push(AuthorityVote { authority: authority_to_vote_on, votes });
        }
    }
    Ok(auth_vote)
}

/// Given a list of votes, return the outcome of the vote based on the majority vote
pub fn get_outcome_of_votes(votes: AuthorityVote) -> Vote {
    let mut add_votes = 0;
    let mut remove_votes = 0;

    for vote in votes.votes {
        match vote.1 {
            Vote::Add => add_votes += 1,
            Vote::Remove => remove_votes += 1,
        }
    }

    if add_votes > remove_votes {
        Vote::Add
    } else {
        Vote::Remove
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_vote_try_from() {
        assert_eq!(Vote::try_from(NONCE_AUTH), Ok(Vote::Add));
        assert_eq!(Vote::try_from(NONCE_DROP), Ok(Vote::Remove));
        assert_eq!(Vote::try_from(0), Err("Invalid u64 value for EIP225 Authority Vote"));
    }
}
