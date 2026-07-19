//! Serve a pre-built SPA from a directory on disk (no embedding, no dev server).

#![deny(missing_docs)]

use crate::SpaOptions;
use axum::Router;
use std::path::PathBuf;
use tower_http::services::{ServeDir, ServeFile};

/// A `Router` whose fallback serves `dir`, falling back to `dir/index.html`
/// for client-side routes.
///
/// Unlike [`embed_router`](crate::embed_router), the on-disk fallback serves the
/// index document for **any** unresolved path — including missing files with an
/// extension such as `/missing.js`, which return `200` with the index HTML
/// rather than `404`. Prefer [`embed_router`](crate::embed_router) if you need
/// missing hashed assets to surface as `404`.
pub fn static_router(dir: impl Into<PathBuf>) -> Router {
  static_router_with(dir, SpaOptions::default())
}

/// Like [`static_router`], with configurable [`SpaOptions`] (e.g. the index file
/// served for `/` and unmatched client-side routes).
///
/// See [`static_router`] for the missing-file (SPA-fallback) behavior, which
/// differs from [`embed_router`](crate::embed_router).
pub fn static_router_with(dir: impl Into<PathBuf>, opts: SpaOptions) -> Router {
  let dir = dir.into();
  let index = dir.join(&opts.index);

  Router::new().fallback_service(ServeDir::new(dir).fallback(ServeFile::new(index)))
}
