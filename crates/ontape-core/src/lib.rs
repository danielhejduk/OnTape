pub mod audio;
pub mod clock;
pub mod deck;
pub mod library;
pub mod player;
pub mod session;
pub mod store;
pub mod tape;

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
