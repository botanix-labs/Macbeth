use rs_merkle::{algorithms::Sha256 as MerkleSha256, MerkleTree};
use sha2::{Digest, Sha256};

use crate::database::Utxo;

pub fn hash_utxo(utxo: &Utxo) -> [u8; 32] {
    let utxo_bytes = serde_cbor::to_vec(utxo).expect("Failed to serialize UTXO");
    Sha256::digest(utxo_bytes).into()
}

pub fn construct_merkle_tree(hashes: &[Vec<u8>]) -> MerkleTree<MerkleSha256> {
    let fixed_size_hashes: Vec<[u8; 32]> =
        hashes.iter().map(|hash| hash.clone().try_into().expect("Hash must be 32 bytes")).collect();
    MerkleTree::from_leaves(&fixed_size_hashes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::database::Utxo;
    use bitcoin::{hashes::Hash, OutPoint, Script, TxOut, Txid};
    use rand::{thread_rng, Rng};

    // Helper function to create a UTXO with random values
    fn create_random_utxo() -> Utxo {
        let mut rng = thread_rng();
        let txid = Txid::from_slice(&rng.gen::<[u8; 32]>()).unwrap();
        let vout = rng.gen_range(0..u32::MAX);
        let value = rng.gen_range(1..1_000_000);
        let script_bytes: Vec<u8> = (0..20).map(|_| rng.gen()).collect();
        let script = Script::from_bytes(script_bytes.as_slice());

        Utxo::new(OutPoint::new(txid, vout), TxOut { value, script_pubkey: script.into() }, None)
    }

    #[test]
    fn test_hash_utxo() {
        let utxo = create_random_utxo();
        let hash = hash_utxo(&utxo);
        assert_ne!(hash, [0u8; 32], "Hash should not be all zeros");
    }

    #[test]
    fn test_construct_merkle_tree() {
        let utxos = vec![create_random_utxo(), create_random_utxo()];
        let hashes: Vec<Vec<u8>> = utxos.iter().map(|utxo| hash_utxo(utxo).to_vec()).collect();
        let merkle_tree = construct_merkle_tree(&hashes);
        let root = merkle_tree.root().expect("Merkle tree should have a root");
        assert_ne!(root, [0u8; 32], "Merkle root should not be all zeros");
    }
}
