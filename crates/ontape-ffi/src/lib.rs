mod error;
mod ontape;
mod types;

pub use error::OnTapeError;
pub use ontape::OnTape;
pub use types::*;

uniffi::setup_scaffolding!();

#[uniffi::export]
pub fn version() -> String {
    ontape_core::version().to_owned()
}
