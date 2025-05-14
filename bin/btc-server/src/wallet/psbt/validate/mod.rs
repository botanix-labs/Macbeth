pub mod data_provider;
mod error;
mod outputs;

use crate::wallet::psbt::PsbtExt;
use data_provider::PsbtDataProvider;

pub trait PsbtValidate: PsbtExt {
    fn validate(data_provider: impl PsbtDataProvider) {}
}
