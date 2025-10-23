pub mod db_impl;

#[cfg(test)]
mod tests {
    use super::db_impl::*;
    use botanix_storage::{tables::create_botanix_tables, BotanixProviderFactory};
    use botanix_tem::{
        foundation::{Error, Foundation, ValidationError},
        test_utils::*,
    };
    use reth_db::DatabaseEnv;
    use std::sync::Arc;

    #[test]
    #[ignore]
    fn foundation_test() {
        // ## Setup database for data.
        let mut db: DatabaseEnv =
            reth_db::init_db("test_foundation_data.db", Default::default()).unwrap();
        create_botanix_tables(&mut db).unwrap();

        let factory = BotanixProviderFactory::new(Arc::new(db));
        let data_layer: WBotanixProviderFactory<_> = factory.into();
        dbg!("db setup");

        // ## Setup database for trie commitments.
        let mut db: DatabaseEnv =
            reth_db::init_db("test_foundation_commits.db", Default::default()).unwrap();
        create_botanix_tables(&mut db).unwrap();

        let factory = BotanixProviderFactory::new(Arc::new(db));
        let commit_layer: WBotanixProviderFactory<_> = factory.into();
        dbg!("layers setup");

        let block_a = gen_bitcoin_hash();
        let block_b = gen_bitcoin_hash();
        let block_c = gen_bitcoin_hash();

        // FOUNDATION: Setup.
        let mut f = Foundation::new(data_layer, commit_layer, block_a, 200, 0).unwrap();
        dbg!("foundation setup");

        let origin_root = f.commitment_root().unwrap();
        dbg!("computed origin root");

        // PROPOSE: Construct an invalid state transition.
        let res_err = f
            .propose_commitments(|c| {
                // INVALID: block_hash: `B`, parent_hash: `C`
                c.insert_bitcoin_header_unchecked(block_b, block_c, 201)?;
                Ok(())
            })
            .unwrap_err();

        dbg!("proposed commitments");

        assert_eq!(res_err, Error::ValidationError(ValidationError::BadBitcoinHeader));

        // Commitment state was RESET accordingly.
        let current_root = f.commitment_root().unwrap();
        assert_eq!(current_root, origin_root);

        // FINALIZE: Finalize an invalid state transition.
        let random_root = gen_foundation_state_root();
        let res_err = f
            .finalize_commitments(random_root, |c| {
                // INVALID: block_hash: `B`, parent_hash: `C`
                c.insert_bitcoin_header_unchecked(block_b, block_c, 201)?;
                Ok(())
            })
            .unwrap_err();

        assert_eq!(res_err, Error::ValidationError(ValidationError::BadBitcoinHeader));

        // Commitment state was RESET accordingly.
        let current_root = f.commitment_root().unwrap();
        assert_eq!(current_root, origin_root);
    }
}
