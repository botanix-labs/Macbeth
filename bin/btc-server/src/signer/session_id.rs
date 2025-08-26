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

// TODO: For backward compatibility, pegout session ID must be 32 and sweep session ID must be 33
//  bytes.

impl SigningSessionId {
    pub fn new(unique_payload: &[u8; 32], session_type: SigningSessionType) -> Self {
        let mut data = [0u8; 33];

        data[0] = session_type as u8;
        data[1..33].copy_from_slice(unique_payload);

        Self(data)
    }

    pub fn new_pegout_session(unique_payload: &[u8; 32]) -> Self {
        Self::new(unique_payload, SigningSessionType::Pegout)
    }

    pub fn new_sweep_session(unique_payload: &[u8; 32]) -> Self {
        Self::new(unique_payload, SigningSessionType::Sweep)
    }

    pub fn unique_payload(&self) -> [u8; 32] {
        let mut payload = [0u8; 32];
        payload.copy_from_slice(&self.0[1..33]);
        payload
    }

    pub fn session_type(&self) -> SigningSessionType {
        match self.0[0] {
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
        self.0.to_vec()
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
        if value.len() != 33 {
            return Err(SigningError::InvalidSigningSessionId);
        }

        let session_type = match value.first().ok_or(SigningError::InvalidSigningSessionId)? {
            0x00 => SigningSessionType::Pegout,
            0x01 => SigningSessionType::Sweep,
            _ => return Err(SigningError::InvalidSigningSessionId),
        };

        let mut session_id_array = [0u8; 32];
        session_id_array.copy_from_slice(&value[1..33]);

        Ok(SigningSessionId::new(&session_id_array, session_type))
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
        &self.0
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
            let session_id = SigningSessionId::new(&payload, SigningSessionType::Pegout);

            assert_eq!(session_id.session_type(), SigningSessionType::Pegout);
            assert_eq!(session_id.unique_payload(), payload);
        }

        #[test]
        fn test_new_with_sweep_type() {
            let payload = [2u8; 32];
            let session_id = SigningSessionId::new(&payload, SigningSessionType::Sweep);

            assert_eq!(session_id.session_type(), SigningSessionType::Sweep);
            assert_eq!(session_id.unique_payload(), payload);
        }
    }

    mod to_vec {
        use super::*;

        #[test]
        fn test_to_vec_pegout() {
            let payload = [42u8; 32];
            let session_id = SigningSessionId::new(&payload, SigningSessionType::Pegout);

            let vec = session_id.to_vec();
            assert_eq!(vec.len(), 33);
            assert_eq!(vec[0], 0x00); // Pegout type
            assert_eq!(&vec[1..33], &payload);
        }

        #[test]
        fn test_to_vec_sweep() {
            let payload = [123u8; 32];
            let session_id = SigningSessionId::new(&payload, SigningSessionType::Sweep);

            let vec = session_id.to_vec();
            assert_eq!(vec.len(), 33);
            assert_eq!(vec[0], 0x01); // Sweep type
            assert_eq!(&vec[1..33], &payload);
        }
    }

    mod into_vec {
        use super::*;

        #[test]
        fn test_pegout_into_vec() {
            let payload = [55u8; 32];
            let session_id = SigningSessionId::new(&payload, SigningSessionType::Pegout);

            let vec: Vec<u8> = session_id.into();
            assert_eq!(vec.len(), 33);
            assert_eq!(vec[0], 0x00);
            assert_eq!(&vec[1..33], &payload);
        }

        #[test]
        fn test_sweep_into_vec() {
            let payload = [77u8; 32];
            let session_id = SigningSessionId::new(&payload, SigningSessionType::Sweep);

            let vec: Vec<u8> = session_id.into();
            assert_eq!(vec.len(), 33);
            assert_eq!(vec[0], 0x01);
            assert_eq!(&vec[1..33], &payload);
        }
    }

    mod try_from_slice {
        use super::*;

        #[test]
        fn test_try_from_valid_pegout() {
            let payload = [111u8; 32];
            let mut data = [0u8; 33];
            data[0] = 0x00; // Pegout
            data[1..33].copy_from_slice(&payload);

            let session_id = SigningSessionId::try_from(data.as_slice()).unwrap();
            assert_eq!(session_id.session_type(), SigningSessionType::Pegout);
            assert_eq!(session_id.unique_payload(), payload);
        }

        #[test]
        fn test_try_from_valid_sweep() {
            let payload = [222u8; 32];
            let mut data = [0u8; 33];
            data[0] = 0x01; // Sweep
            data[1..33].copy_from_slice(&payload);

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
            let data = [0u8; 32]; // Only 32 bytes instead of 33
            let result = SigningSessionId::try_from(data.as_slice());
            assert!(matches!(result, Err(SigningError::InvalidSigningSessionId)));
        }

        #[test]
        fn test_try_from_too_long() {
            let data = [0u8; 34]; // 34 bytes instead of 33
            let result = SigningSessionId::try_from(data.as_slice());
            assert!(matches!(result, Err(SigningError::InvalidSigningSessionId)));
        }

        #[test]
        fn test_try_from_invalid_session_type() {
            let mut data = [0u8; 33];
            data[0] = 0x02; // Invalid session type

            let result = SigningSessionId::try_from(data.as_slice());
            assert!(matches!(result, Err(SigningError::InvalidSigningSessionId)));
        }

        #[test]
        fn test_try_from_max_invalid_session_type() {
            let mut data = [0u8; 33];
            data[0] = 255; // Invalid session type

            let result = SigningSessionId::try_from(data.as_slice());
            assert!(matches!(result, Err(SigningError::InvalidSigningSessionId)));
        }

        #[test]
        fn test_try_from_roundtrip_pegout() {
            let payload = [42u8; 32];
            let original = SigningSessionId::new(&payload, SigningSessionType::Pegout);
            let vec = original.to_vec();
            let reconstructed = SigningSessionId::try_from(vec.as_slice()).unwrap();

            assert_eq!(original.session_type(), reconstructed.session_type());
            assert_eq!(original.unique_payload(), reconstructed.unique_payload());
        }

        #[test]
        fn test_try_from_roundtrip_sweep() {
            let payload = [84u8; 32];
            let original = SigningSessionId::new(&payload, SigningSessionType::Sweep);
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
            let session_id = SigningSessionId::new(&payload, SigningSessionType::Pegout);

            let slice: &[u8] = session_id.as_ref();
            assert_eq!(slice.len(), 33);
            assert_eq!(slice[0], 0x00); // Pegout type
            assert_eq!(&slice[1..33], &payload);
        }

        #[test]
        fn test_as_ref_sweep() {
            let payload = [97u8; 32];
            let session_id = SigningSessionId::new(&payload, SigningSessionType::Sweep);

            let slice: &[u8] = session_id.as_ref();
            assert_eq!(slice.len(), 33);
            assert_eq!(slice[0], 0x01); // Sweep type
            assert_eq!(&slice[1..33], &payload);
        }
    }
}
