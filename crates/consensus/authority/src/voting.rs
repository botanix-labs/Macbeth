use reth_primitives::{
    constants::eip225::{NONCE_AUTH, NONCE_DROP},
    Header,
};
use std::collections::HashMap;

use botanix_lib::extra_data_header::{ExtraDataHeader, ExtraDataHeaderDeserialzeError};

use crate::utils::create_authority_sighash;

/// Repersenting a vote to add or remove an authority
pub enum Vote {
    Add,
    Remove,
}

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
struct AuthorityVote {
    /// Authority to add/remove
    authority: secp256k1::PublicKey,
    /// Votes for this authority
    votes: HashMap<secp256k1::PublicKey, Vote>,
}

pub enum GetVotesError {
    FailedToDeserializeBlockHeaderExtraData(ExtraDataHeaderDeserialzeError),
    FailedToRecoverAuthority(secp256k1::Error),
    FailedToParseNonceVote,
}

/// Given a range of block headers we want a utility function that will return a list of votes
pub fn get_vote_results(headers: Vec<Header>) -> Result<Vec<AuthorityVote>, GetVotesError> {
    // Structure to keep track of all votes that occured in this block range
    let mut auth_vote: Vec<AuthorityVote> = Vec::new();

    for header in headers {
        if header.is_empty() {
            continue
        }
        // Check if there is a authority being voted on in the extra data
        let extra_data_header = ExtraDataHeader::deserialize(header.extra_data.as_slice())
            .map_err(|e| GetVotesError::FailedToDeserializeBlockHeaderExtraData(e))?;

        if extra_data_header.authority_vote.is_none() {
            continue
        }

        if extra_data_header.authority_signature.is_none() {
            continue
        }

        // Check if there is a valid vote in the nonce field
        if header.nonce != NONCE_AUTH && header.nonce != NONCE_DROP {
            continue
        }

        let authority = extra_data_header.authority_vote.expect("valid authority vote");
        // Already keeping track of this authority
        if auth_vote.contains(&authority) {
            // Need to recover the authority that signed the block from the signature
            let sig_hash = secp256k1::Message::from_slice(
                create_authority_sighash(&header, &extra_data_header).unwrap().as_slice(),
            )
            .map_err(|e| GetVotesError::FailedToDeserializeBlockHeaderExtraData(e))?;

            let authority_that_votes = extra_data_header
                .authority_signature
                .expect("valid signature")
                .recover(&sig_hash)
                .map_err(|e| GetVotesError::FailedToRecoverAuthority(e))?;

            // Check if the authority that signed block currently has a vote for this authority
            let current_vote = auth_vote.iter().find(|vote: &&AuthorityVote| vote == authority);

            // Check if the block producer already provided a vote for this authority
            if current_vote.expect("valid vote").votes.contains_key(&authority_that_votes) {
                continue
            }

            current_vote.expect("valid vote").votes.insert(
                authority_that_votes,
                header.nonce.try_into().map_err(|| GetVotesError::FailedToParseNonceVote)?,
            );
        } else {
            let mut votes = HashMap::new();
            votes.insert(
                authority_that_votes,
                header.nonce.try_into().map_err(|| GetVotesError::FailedToParseNonceVote)?,
            );
            auth_vote.push(AuthorityVote { authority, votes });
        }
    }
}

/// Given a list of votes, return the outcome of the vote based on the majority vote
pub fn get_outcome_of_vote(votes: AuthorityVote) -> Vote {
    let mut add_votes = 0;
    let mut remove_votes = 0;

    for vote in votes.votes {
        match vote {
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


