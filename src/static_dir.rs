//! Serve a pre-built SPA from a directory on disk (no embedding, no dev server).

#![deny(missing_docs)]

use crate::spa::names_a_file;
use crate::SpaOptions;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::Router;
use std::path::PathBuf;
use tower_http::services::{ServeDir, ServeFile};

/// A `Router` whose fallback serves `dir`, falling back to `dir/index.html`
/// for client-side routes.
///
/// As with [`embed_router`](crate::embed_router), a missing path with a dot in it
/// such as `/missing.js` is a `404`, and a directory path such as `/assets` is a
/// client-side route, so an `index.html` inside a subdirectory is not served for
/// it. See [`SpaOptions::apply`] for the policy this router applies.
pub fn static_router(dir: impl Into<PathBuf>) -> Router {
  static_router_with(dir, SpaOptions::default())
}

/// Like [`static_router`], with configurable [`SpaOptions`] (e.g. the index file
/// served for `/` and unmatched client-side routes).
pub fn static_router_with(dir: impl Into<PathBuf>, opts: SpaOptions) -> Router {
  let dir = dir.into();
  let client_routes = Router::new()
    .fallback_service(ServeFile::new(dir.join(&opts.index)))
    .layer(middleware::from_fn(reject_missing_file));
  // Left on, ServeDir answers `/` with `index.html` itself, ignoring `opts.index`.
  let served = ServeDir::new(dir)
    .append_index_html_on_directories(false)
    .fallback(client_routes);
  let files = Router::new().fallback_service(served);

  opts.apply(files)
}

async fn reject_missing_file(request: Request, next: Next) -> Response {
  if names_a_file(request.uri().path()) {
    return StatusCode::NOT_FOUND.into_response();
  }

  next.run(request).await
}
