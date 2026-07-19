//! Shared options for the SPA-serving routers (`embed` and `static`).

#![deny(missing_docs)]

/// Options shared by the embed and static SPA routers.
#[derive(Debug, Clone)]
pub struct SpaOptions {
  pub(crate) index: String,
}

impl SpaOptions {
  /// Defaults: index document `index.html`.
  #[must_use]
  pub fn new() -> Self {
    Self {
      index: "index.html".to_string(),
    }
  }

  /// Set the SPA entry document served for `/` and unmatched client-side routes.
  #[must_use]
  pub fn index(mut self, name: impl Into<String>) -> Self {
    self.index = name.into();

    self
  }
}

impl Default for SpaOptions {
  fn default() -> Self {
    Self::new()
  }
}
