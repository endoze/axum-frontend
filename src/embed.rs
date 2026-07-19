//! Serve a pre-built SPA embedded in the binary via `rust-embed`.

#![deny(missing_docs)]

use crate::SpaOptions;
use axum::http::{header, StatusCode, Uri};
use axum::response::{Html, IntoResponse, Response};
use axum::Router;
use rust_embed::RustEmbed;
use std::sync::Arc;

/// A `Router` whose fallback serves the embedded assets of `E`, using
/// `index.html` as the SPA entry document.
///
/// Merge this into your app's router; your explicit routes take precedence and
/// everything else is served from the binary. Unknown extension-less paths fall
/// back to the index document so client-side routing works.
pub fn embed_router<E: RustEmbed + 'static>() -> Router {
  embed_router_with::<E>(SpaOptions::default())
}

/// Like [`embed_router`], with configurable [`SpaOptions`] (e.g. the index file).
pub fn embed_router_with<E: RustEmbed + 'static>(opts: SpaOptions) -> Router {
  let index: Arc<str> = Arc::from(opts.index);

  Router::new().fallback(move |uri: Uri| {
    let index = Arc::clone(&index);

    async move { serve_embedded::<E>(uri, index.as_ref()) }
  })
}

fn serve_embedded<E: RustEmbed + 'static>(uri: Uri, index: &str) -> Response {
  let path = uri.path().trim_start_matches('/');

  if path.is_empty() || path == index {
    return index_html::<E>(index);
  }

  match E::get(path) {
    Some(content) => (
      [(header::CONTENT_TYPE, content.metadata.mimetype())],
      content.data,
    )
      .into_response(),
    None => {
      if path.contains('.') {
        return StatusCode::NOT_FOUND.into_response();
      }

      index_html::<E>(index)
    }
  }
}

fn index_html<E: RustEmbed + 'static>(index: &str) -> Response {
  match E::get(index) {
    Some(content) => Html(content.data).into_response(),
    None => StatusCode::NOT_FOUND.into_response(),
  }
}
