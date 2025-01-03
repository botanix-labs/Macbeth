#[allow(unused)]

use reth_primitives::BlockNumber;
use thiserror::Error;

/// Activation status
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActivationStatus {
    /// The activation is defined
    Defined,
    /// The activation voting period has started
    VotingStarted,
    /// The activation is active
    Active,
    /// The activation is failed
    Failed,
}

/// Activation params
#[derive(Debug, Clone)]
pub struct ActivationParams {
    /// The name of the activation
    name: String,
    /// The start height of the activation
    start_height: BlockNumber,
    /// The end height of the activation
    end_height: BlockNumber,
    /// The vote bit number
    vote_bit_number: u16,
    /// How many blocks need to signal the voting bit to activate
    threshold: u64,
}

/// Activation error
#[derive(Debug, Clone, PartialEq, Error)]
pub enum ActivationError {
    /// Invalid vote bit number
    #[error("Invalid vote bit number")]
    InvalidVoteBitNumber,
    /// Invalid threshold
    #[error("Invalid threshold")]
    InvalidThreshold,
    /// Invalid start height
    #[error("Invalid start height")]
    InvalidStartHeight,
}

impl ActivationParams {
    /// Create a new activation params
    pub fn try_new(
        name: String,
        start_height: BlockNumber,
        end_height: BlockNumber,
        vote_bit_number: u16,
        threshold: u64,
    ) -> Result<Self, ActivationError> {
        if vote_bit_number >= 16 {
            return Err(ActivationError::InvalidVoteBitNumber);
        }

        if start_height > end_height {
            return Err(ActivationError::InvalidStartHeight);
        }

        if threshold > end_height.saturating_sub(start_height) {
            return Err(ActivationError::InvalidThreshold);
        }

        Ok(Self { name, start_height, end_height, vote_bit_number, threshold })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_activation_params() {
        // attempt to create an activation params with an invalid vote bit number
        let result = ActivationParams::try_new("test".to_string(), 100, 200, 16, 10);

        assert_eq!(result.err().unwrap(), ActivationError::InvalidVoteBitNumber);

        // attempt to create an activation params with an invalid threshold
        let result = ActivationParams::try_new("test".to_string(), 100, 200, 0, 201);
        assert_eq!(result.err().unwrap(), ActivationError::InvalidThreshold);

        // attempt to create an activation params with an invalid start height
        let result = ActivationParams::try_new("test".to_string(), 200, 100, 0, 10);
        assert_eq!(result.err().unwrap(), ActivationError::InvalidStartHeight);
    }
}
