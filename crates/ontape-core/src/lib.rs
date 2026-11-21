mod player;
pub mod tape;

pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
