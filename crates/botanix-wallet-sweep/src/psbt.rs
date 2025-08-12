use crate::request::WalletSweepRequest;
use bitcoin::Psbt;

pub fn create_psbt(_request: WalletSweepRequest) -> eyre::Result<Psbt> {
    todo!("Implement PSBT creation logic based on the WalletSweepRequest");
}
