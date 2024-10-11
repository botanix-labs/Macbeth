use crate::suite::consensus::ConsensusIntegrationTestSuite;

pub async fn test_bitcoind_networking(
    _suite: &ConsensusIntegrationTestSuite,
) -> anyhow::Result<(), super::error::Error> {
    // TODO: Implement bitcoind networking test
    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    Ok(())
}
