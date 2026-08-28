//!
#![doc = include_str!("../README.md")]
#![deny(missing_docs)]

mod embed;
pub use embed::{embed_router, embed_router_with};
pub use rust_embed::{Embed, RustEmbed};

mod spa;
pub use spa::SpaOptions;

mod static_dir;
pub use static_dir::{static_router, static_router_with};

#[cfg(feature = "dev")]
mod dev;
#[cfg(feature = "dev")]
pub use dev::{dev_router, shutdown_requested, wait_until_ready, DevConfig, DevServer};

#[cfg(test)]
mod tests {
  // Smoke test: if this compiles and runs, the crate built for the active
  // feature set. Per-feature behavior is covered by the integration tests.
  #[test]
  fn crate_builds() {}
}
