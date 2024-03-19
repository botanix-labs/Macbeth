use hex::{self, encode as hex_encode};
use jsonwebtoken::{decode, errors::ErrorKind, Algorithm, DecodingKey, Validation};
use rand::Rng;
use serde::{Deserialize, Serialize};
use std::{
    fs,
    path::{Path, PathBuf},
    str::FromStr,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use thiserror::Error;

/// Errors returned by the [`JwtSecret`]
#[derive(Error, Debug)]
pub enum JwtError {
    /// An error encountered while decoding the hexadecimal string for the JWT secret.
    #[error(transparent)]
    JwtSecretHexDecodeError(#[from] hex::FromHexError),

    /// The JWT key length provided is invalid, expecting a specific length.
    #[error("JWT key is expected to have a length of {0} digits. {1} digits key provided")]
    InvalidLength(usize, usize),

    /// The signature algorithm used in the JWT is not supported. Only HS256 is supported.
    #[error("unsupported signature algorithm. Only HS256 is supported")]
    UnsupportedSignatureAlgorithm,

    /// The provided signature in the JWT is invalid.
    #[error("provided signature is invalid")]
    InvalidSignature,

    /// The "iat" (issued-at) claim in the JWT is not within the allowed ±60 seconds from the
    /// current time.
    #[error("IAT (issued-at) claim is not within ±60 seconds from the current time")]
    InvalidIssuanceTimestamp,

    /// The Authorization header is missing or invalid in the context of JWT validation.
    #[error("Authorization header is missing or invalid")]
    MissingOrInvalidAuthorizationHeader,

    /// An error occurred during JWT decoding.
    #[error("JWT decoding error: {0}")]
    JwtDecodingError(String),

    /// An I/O error occurred during JWT operations.
    #[error(transparent)]
    IOError(#[from] std::io::Error),
}

/// Length of the hex-encoded 256 bit secret key.
/// A 256-bit encoded string in Rust has a length of 64 digits because each digit represents 4 bits
/// of data. In hexadecimal representation, each digit can have 16 possible values (0-9 and A-F), so
/// 4 bits can be represented using a single hex digit. Therefore, to represent a 256-bit string,
/// we need 64 hexadecimal digits (256 bits ÷ 4 bits per digit = 64 digits).
const JWT_SECRET_LEN: usize = 64;

/// The JWT `iat` (issued-at) claim cannot exceed +-60 seconds from the current time.
const JWT_MAX_IAT_DIFF: Duration = Duration::from_secs(60);

/// The execution layer client MUST support at least the following alg HMAC + SHA256 (HS256)
const JWT_SIGNATURE_ALGO: Algorithm = Algorithm::HS256;

#[derive(Clone, PartialEq, Eq)]
pub struct JwtSecret(pub [u8; 32]);

/// Attempts to retrieve or create a JWT secret from the specified path.
pub fn get_or_create_jwt_secret_from_path(path: &Path) -> Result<JwtSecret, JwtError> {
    if path.exists() {
        JwtSecret::from_file(path)
    } else {
        JwtSecret::try_create(path)
    }
}

impl JwtSecret {
    /// Creates an instance of [`JwtSecret`].
    ///
    /// Returns an error if one of the following applies:
    /// - `hex` is not a valid hexadecimal string
    /// - `hex` argument length is less than `JWT_SECRET_LEN`
    ///
    /// This strips the leading `0x`, if any.
    pub fn from_hex<S: AsRef<str>>(hex: S) -> Result<Self, JwtError> {
        let hex: &str = hex.as_ref().trim().trim_start_matches("0x");
        if hex.len() != JWT_SECRET_LEN {
            Err(JwtError::InvalidLength(JWT_SECRET_LEN, hex.len()))
        } else {
            let hex_bytes = hex::decode(hex)?;
            // is 32bytes, see length check
            let bytes = hex_bytes.try_into().expect("is expected len");
            Ok(JwtSecret(bytes))
        }
    }

    /// Tries to load a [`JwtSecret`] from the specified file path.
    /// I/O or secret validation errors might occur during read operations in the form of
    /// a [`JwtError`].
    pub fn from_file(fpath: &Path) -> Result<Self, JwtError> {
        let hex = fs::read_to_string(fpath)?;
        let secret = JwtSecret::from_hex(hex)?;
        Ok(secret)
    }

    /// Creates a random [`JwtSecret`] and tries to store it at the specified path. I/O errors might
    /// occur during write operations in the form of a [`JwtError`]
    pub fn try_create(fpath: &Path) -> Result<Self, JwtError> {
        if let Some(dir) = fpath.parent() {
            // Create parent directory
            fs::create_dir_all(dir)?
        }

        let secret = JwtSecret::random();
        let bytes = &secret.0;
        let hex = hex::encode(bytes);
        fs::write(fpath, hex)?;
        Ok(secret)
    }

    /// Validates a JWT token along the following rules:
    /// - The JWT signature is valid.
    /// - The JWT is signed with the `HMAC + SHA256 (HS256)` algorithm.
    /// - The JWT `iat` (issued-at) claim is a timestamp within +-60 seconds from the current time.
    ///
    /// See also: [JWT Claims - Engine API specs](https://github.com/ethereum/execution-apis/blob/main/src/engine/authentication.md#jwt-claims)
    pub fn validate(&self, jwt: String) -> Result<(), JwtError> {
        let mut validation = Validation::new(JWT_SIGNATURE_ALGO);
        // ensure that the JWT has an `iat` claim
        validation.set_required_spec_claims(&["iat"]);
        let bytes = &self.0;

        match decode::<Claims>(&jwt, &DecodingKey::from_secret(bytes), &validation) {
            Ok(token) => {
                if !token.claims.is_within_time_window() {
                    Err(JwtError::InvalidIssuanceTimestamp)?
                }
            }
            Err(err) => match *err.kind() {
                ErrorKind::InvalidSignature => Err(JwtError::InvalidSignature)?,
                ErrorKind::InvalidAlgorithm => Err(JwtError::UnsupportedSignatureAlgorithm)?,
                _ => {
                    let detail = format!("{err}");
                    Err(JwtError::JwtDecodingError(detail))?
                }
            },
        };

        Ok(())
    }

    /// Generates a random [`JwtSecret`] containing a hex-encoded 256 bit secret key.
    pub fn random() -> Self {
        let random_bytes: [u8; 32] = rand::thread_rng().gen();
        let secret = hex_encode(random_bytes);
        JwtSecret::from_hex(secret).unwrap()
    }

    /// Encode the header and claims given and sign the payload using the algorithm from the header
    /// and the key.
    ///
    /// ```rust
    /// use reth_rpc::{Claims, JwtSecret};
    ///
    /// let my_claims = Claims { iat: 0, exp: None };
    /// let secret = JwtSecret::random();
    /// let token = secret.encode(&my_claims).unwrap();
    /// ```
    pub fn encode(&self, claims: &Claims) -> Result<String, jsonwebtoken::errors::Error> {
        let bytes = &self.0;
        let key = jsonwebtoken::EncodingKey::from_secret(bytes);
        let algo = jsonwebtoken::Header::new(Algorithm::HS256);
        jsonwebtoken::encode(&algo, claims, &key)
    }
}

impl std::fmt::Debug for JwtSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("JwtSecretHash").field(&"{{}}").finish()
    }
}

impl FromStr for JwtSecret {
    type Err = JwtError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        JwtSecret::from_hex(s)
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Claims {
    /// The "iat" value MUST be a number containing a NumericDate value.
    /// According to the RFC A NumericDate represents the number of seconds since
    /// the UNIX_EPOCH.
    /// - [`RFC-7519 - Spec`](https://www.rfc-editor.org/rfc/rfc7519#section-4.1.6)
    /// - [`RFC-7519 - Notations`](https://www.rfc-editor.org/rfc/rfc7519#section-2)
    pub iat: u64,
    /// Expiration, if any
    pub exp: Option<u64>,
}

impl Claims {
    fn is_within_time_window(&self) -> bool {
        let now = SystemTime::now();
        let now_secs = now.duration_since(UNIX_EPOCH).unwrap().as_secs();
        now_secs.abs_diff(self.iat) <= JWT_MAX_IAT_DIFF.as_secs()
    }
}
