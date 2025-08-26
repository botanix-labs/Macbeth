use crate::signer::error::SigningError;
use std::fmt::Display;

#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SigningSessionType {
    Pegout = 0x00,
    Sweep = 0x01,
}

// Store as [type(1 byte), payload(32 bytes)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SigningSessionId([u8; 33]);

impl SigningSessionId {
    pub fn new(unique_payload: [u8; 32], session_type: SigningSessionType) -> Self {
        let mut data = [0u8; 33];

        data[0..32].copy_from_slice(&unique_payload);
        data[32] = session_type as u8;

        Self(data)
    }

    pub fn new_pegout_session(unique_payload: [u8; 32]) -> Self {
        Self::new(unique_payload, SigningSessionType::Pegout)
    }

    pub fn new_sweep_session(unique_payload: [u8; 32]) -> Self {
        Self::new(unique_payload, SigningSessionType::Sweep)
    }

    pub fn unique_payload(&self) -> [u8; 32] {
        let mut payload = [0u8; 32];
        payload.copy_from_slice(&self.0[0..32]);
        payload
    }

    pub fn session_type(&self) -> SigningSessionType {
        match self.0[32] {
            0x00 => SigningSessionType::Pegout,
            0x01 => SigningSessionType::Sweep,
            _ => unreachable!("Invalid session type in data"),
        }
    }

    pub fn is_sweep_session(&self) -> bool {
        self.session_type() == SigningSessionType::Sweep
    }

    pub fn is_pegout_session(&self) -> bool {
        self.session_type() == SigningSessionType::Pegout
    }

    pub fn to_vec(&self) -> Vec<u8> {
        if self.is_pegout_session() {
            &self.0[0..32] // Pegout session ID is 32 bytes for backward compatibility
        } else {
            &self.0 // Sweep session ID is 33 bytes
        }
        .to_vec()
    }
}

impl From<SigningSessionId> for Vec<u8> {
    fn from(value: SigningSessionId) -> Self {
        value.to_vec()
    }
}

impl TryFrom<&[u8]> for SigningSessionId {
    type Error = SigningError;

    fn try_from(value: &[u8]) -> Result<Self, Self::Error> {
        // Session ID normally is 33 bytes: 32 bytes for a unique payload + 1 byte for type
        // Pegout session ID is 32 bytes for backward compatibility
        let session_type = if value.len() == 32 {
            SigningSessionType::Pegout
        } else if value.len() == 33 {
            match value.last().ok_or(SigningError::InvalidSigningSessionId)? {
                0x00 => SigningSessionType::Pegout,
                0x01 => SigningSessionType::Sweep,
                _ => return Err(SigningError::InvalidSigningSessionId),
            }
        } else {
            return Err(SigningError::InvalidSigningSessionId);
        };

        let mut session_id_array = [0u8; 32];
        session_id_array.copy_from_slice(&value[0..32]);

        Ok(SigningSessionId::new(session_id_array, session_type))
    }
}

impl TryFrom<Vec<u8>> for SigningSessionId {
    type Error = SigningError;

    fn try_from(value: Vec<u8>) -> Result<Self, Self::Error> {
        SigningSessionId::try_from(value.as_slice())
    }
}

impl AsRef<[u8]> for SigningSessionId {
    fn as_ref(&self) -> &[u8] {
        if self.is_pegout_session() {
            &self.0[0..32] // Pegout session ID is 32 bytes for backward compatibility
        } else {
            &self.0 // Sweep session ID is 33 bytes
        }
    }
}

impl Display for SigningSessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", hex::encode(self.0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    mod new {
        use super::*;

        #[test]
        fn test_new_with_pegout_type() {
            let payload = [1u8; 32];
            let session_id = SigningSessionId::new(payload, SigningSessionType::Pegout);

            assert_eq!(session_id.session_type(), SigningSessionType::Pegout);
            assert_eq!(session_id.unique_payload(), payload);
        }

        #[test]
        fn test_new_with_sweep_type() {
            let payload = [2u8; 32];
            let session_id = SigningSessionId::new(payload, SigningSessionType::Sweep);

            assert_eq!(session_id.session_type(), SigningSessionType::Sweep);
            assert_eq!(session_id.unique_payload(), payload);
        }
    }

    mod new_pegout_session {
        use super::*;

        #[test]
        fn test_new_pegout_session() {
            let payload = [123u8; 32];
            let session_id = SigningSessionId::new_pegout_session(payload);

            assert_eq!(session_id.session_type(), SigningSessionType::Pegout);
            assert_eq!(session_id.unique_payload(), payload);
        }
    }

    mod new_sweep_session {
        use super::*;

        #[test]
        fn test_new_sweep_session() {
            let payload = [67u8; 32];
            let session_id = SigningSessionId::new_sweep_session(payload);

            assert_eq!(session_id.session_type(), SigningSessionType::Sweep);
            assert_eq!(session_id.unique_payload(), payload);
        }
    }

    mod is_sweep_session {
        use super::*;

        #[test]
        fn test_is_sweep_session_true() {
            let payload = [11u8; 32];
            let session_id = SigningSessionId::new(payload, SigningSessionType::Sweep);

            assert!(session_id.is_sweep_session());
            assert!(!session_id.is_pegout_session());
        }

        #[test]
        fn test_is_sweep_session_false() {
            let payload = [22u8; 32];
            let session_id = SigningSessionId::new(payload, SigningSessionType::Pegout);

            assert!(!session_id.is_sweep_session());
            assert!(session_id.is_pegout_session());
        }
    }

    mod is_pegout_session {
        use super::*;

        #[test]
        fn test_is_pegout_session_true() {
            let payload = [33u8; 32];
            let session_id = SigningSessionId::new(payload, SigningSessionType::Pegout);

            assert!(session_id.is_pegout_session());
            assert!(!session_id.is_sweep_session());
        }

        #[test]
        fn test_is_pegout_session_false() {
            let payload = [44u8; 32];
            let session_id = SigningSessionId::new(payload, SigningSessionType::Sweep);

            assert!(!session_id.is_pegout_session());
            assert!(session_id.is_sweep_session());
        }
    }

    mod to_vec {
        use super::*;

        #[test]
        fn test_to_vec_pegout() {
            let payload = [42u8; 32];
            let session_id = SigningSessionId::new(payload, SigningSessionType::Pegout);

            let vec = session_id.to_vec();
            assert_eq!(vec.len(), 32); // Pegout returns only 32 bytes for backward compatibility
            assert_eq!(vec, payload.to_vec());
        }

        #[test]
        fn test_to_vec_sweep() {
            let payload = [123u8; 32];
            let session_id = SigningSessionId::new(payload, SigningSessionType::Sweep);

            let vec = session_id.to_vec();
            assert_eq!(vec.len(), 33); // Sweep returns full 33 bytes
            assert_eq!(&vec[0..32], &payload);
            assert_eq!(vec[32], 0x01); // Sweep type
        }

        #[test]
        fn test_to_vec_returns_new_vec() {
            let payload = [99u8; 32];
            let session_id = SigningSessionId::new(payload, SigningSessionType::Pegout);

            let mut vec1 = session_id.to_vec();
            let vec2 = session_id.to_vec();

            vec1[0] = 255; // Modify first vec
            assert_ne!(vec1, vec2); // Should be independent
        }
    }

    mod into_vec {
        use super::*;

        #[test]
        fn test_pegout_into_vec() {
            let payload = [55u8; 32];
            let session_id = SigningSessionId::new(payload, SigningSessionType::Pegout);

            let vec: Vec<u8> = session_id.into();
            assert_eq!(vec.len(), 32); // Pegout returns 32 bytes
            assert_eq!(vec, payload.to_vec());
        }

        #[test]
        fn test_sweep_into_vec() {
            let payload = [77u8; 32];
            let session_id = SigningSessionId::new(payload, SigningSessionType::Sweep);

            let vec: Vec<u8> = session_id.into();
            assert_eq!(vec.len(), 33); // Sweep returns 33 bytes
            assert_eq!(&vec[0..32], &payload);
            assert_eq!(vec[32], 0x01);
        }
    }

    mod try_from_slice {
        use super::*;

        #[test]
        fn test_try_from_valid_pegout_33_bytes() {
            let payload = [111u8; 32];
            let mut data = [0u8; 33];
            data[0..32].copy_from_slice(&payload);
            data[32] = 0x00; // Pegout type

            let session_id = SigningSessionId::try_from(data.as_slice()).unwrap();
            assert_eq!(session_id.session_type(), SigningSessionType::Pegout);
            assert_eq!(session_id.unique_payload(), payload);
        }

        #[test]
        fn test_try_from_valid_pegout_32_bytes_legacy() {
            let payload = [111u8; 32];

            let session_id = SigningSessionId::try_from(payload.as_slice()).unwrap();
            assert_eq!(session_id.session_type(), SigningSessionType::Pegout);
            assert_eq!(session_id.unique_payload(), payload);
        }

        #[test]
        fn test_try_from_valid_sweep() {
            let payload = [222u8; 32];
            let mut data = [0u8; 33];
            data[0..32].copy_from_slice(&payload);
            data[32] = 0x01; // Sweep type

            let session_id = SigningSessionId::try_from(data.as_slice()).unwrap();
            assert_eq!(session_id.session_type(), SigningSessionType::Sweep);
            assert_eq!(session_id.unique_payload(), payload);
        }

        #[test]
        fn test_try_from_empty_slice() {
            let data: &[u8] = &[];
            let result = SigningSessionId::try_from(data);
            assert!(matches!(result, Err(SigningError::InvalidSigningSessionId)));
        }

        #[test]
        fn test_try_from_too_short() {
            let data = [0u8; 31]; // Only 31 bytes
            let result = SigningSessionId::try_from(data.as_slice());
            assert!(matches!(result, Err(SigningError::InvalidSigningSessionId)));
        }

        #[test]
        fn test_try_from_too_long() {
            let data = [0u8; 34]; // 34 bytes instead of 32 or 33
            let result = SigningSessionId::try_from(data.as_slice());
            assert!(matches!(result, Err(SigningError::InvalidSigningSessionId)));
        }

        #[test]
        fn test_try_from_invalid_session_type() {
            let mut data = [0u8; 33];
            data[32] = 0x02; // Invalid session type

            let result = SigningSessionId::try_from(data.as_slice());
            assert!(matches!(result, Err(SigningError::InvalidSigningSessionId)));
        }

        #[test]
        fn test_try_from_max_invalid_session_type() {
            let mut data = [0u8; 33];
            data[32] = 255; // Invalid session type

            let result = SigningSessionId::try_from(data.as_slice());
            assert!(matches!(result, Err(SigningError::InvalidSigningSessionId)));
        }

        #[test]
        fn test_try_from_roundtrip_pegout() {
            let payload = [42u8; 32];
            let original = SigningSessionId::new(payload, SigningSessionType::Pegout);
            let vec = original.to_vec();
            let reconstructed = SigningSessionId::try_from(vec.as_slice()).unwrap();

            assert_eq!(original.session_type(), reconstructed.session_type());
            assert_eq!(original.unique_payload(), reconstructed.unique_payload());
        }

        #[test]
        fn test_try_from_roundtrip_sweep() {
            let payload = [84u8; 32];
            let original = SigningSessionId::new(payload, SigningSessionType::Sweep);
            let vec = original.to_vec();
            let reconstructed = SigningSessionId::try_from(vec.as_slice()).unwrap();

            assert_eq!(original.session_type(), reconstructed.session_type());
            assert_eq!(original.unique_payload(), reconstructed.unique_payload());
        }
    }

    mod as_ref {
        use super::*;

        #[test]
        fn test_as_ref_pegout() {
            let payload = [13u8; 32];
            let session_id = SigningSessionId::new(payload, SigningSessionType::Pegout);

            let slice: &[u8] = session_id.as_ref();
            assert_eq!(slice.len(), 32); // Pegout returns only 32 bytes
            assert_eq!(slice, &payload);
        }

        #[test]
        fn test_as_ref_sweep() {
            let payload = [97u8; 32];
            let session_id = SigningSessionId::new(payload, SigningSessionType::Sweep);

            let slice: &[u8] = session_id.as_ref();
            assert_eq!(slice.len(), 33); // Sweep returns full 33 bytes
            assert_eq!(&slice[0..32], &payload);
            assert_eq!(slice[32], 0x01); // Sweep type
        }
    }
}
