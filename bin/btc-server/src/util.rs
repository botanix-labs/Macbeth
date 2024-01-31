use crate::Error;
use bitcoin::{consensus::encode as btcencode, hashes::Hash, OutPoint};
use frost_secp256k1_tr as frost;

/// Extension trait for OutPoint.
pub trait OutPointExt: Into<OutPoint> {
    fn to_bytes(self) -> [u8; 36] {
        let OutPoint { txid, vout } = self.into();
        let mut ret = [0u8; 36];
        ret[0..32].copy_from_slice(&txid[..]);
        ret[32..].copy_from_slice(&vout.to_le_bytes()[..]);
        ret
    }

    fn from_bytes(b: [u8; 36]) -> OutPoint {
        btcencode::deserialize(&b).expect("always deserializes")
    }

    fn from_slice(b: &[u8]) -> Result<OutPoint, btcencode::Error> {
        btcencode::deserialize(&b)
    }

    // stopgap for dealing with BDK with other rust-bitcoin version
    fn to_bdk(self) -> bdk::bitcoin::OutPoint {
        let OutPoint { txid, vout } = self.into();
        bdk::bitcoin::OutPoint {
            txid: bdk::bitcoin::hashes::Hash::from_slice(&txid.to_byte_array()).unwrap(),
            vout,
        }
    }

    fn from_bdk(outpoint: bdk::bitcoin::OutPoint) -> OutPoint {
        bitcoin::OutPoint { txid: outpoint.txid, vout: outpoint.vout }
    }
}

impl OutPointExt for OutPoint {}

pub fn serialize_frost_peer_id(id: Vec<u8>) -> Result<frost::Identifier, Error> {
    if id.len() != 32 {
        return Err(Error::InvalidFrostPeerId);
    }
    let peer_id_bytes: &[u8; 32] =
        id.as_slice().try_into().map_err(|_e| Error::InvalidFrostPeerId)?;

    let frost_id =
        frost::Identifier::deserialize(&peer_id_bytes).map_err(|_e| Error::InvalidFrostPeerId)?;

    Ok(frost_id)
}
