//! Serve a pre-built SPA embedded in the binary via `rust-embed`.

#![deny(missing_docs)]

use crate::spa::names_a_file;
use crate::SpaOptions;
use axum::http::{header, HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::Router;
use percent_encoding::percent_decode_str;
use rust_embed::{EmbeddedFile, RustEmbed};
use std::sync::Arc;

const HTML: &str = "text/html; charset=utf-8";

/// A `Router` whose fallback serves the embedded assets of `E`, using
/// `index.html` as the SPA entry document.
///
/// Merge this into your app's router; your explicit routes take precedence and
/// everything else is served from the binary. Unknown extension-less paths fall
/// back to the index document so client-side routing works, and a missing path
/// with a dot in it is a `404`.
///
/// Files carry an `ETag` and answer a matching `If-None-Match` with `304`. See
/// [`SpaOptions::apply`] for the policy this router applies.
pub fn embed_router<E: RustEmbed + 'static>() -> Router {
  embed_router_with::<E>(SpaOptions::default())
}

/// Like [`embed_router`], with configurable [`SpaOptions`] (e.g. the index file).
pub fn embed_router_with<E: RustEmbed + 'static>(opts: SpaOptions) -> Router {
  let index: Arc<str> = Arc::from(opts.index.as_str());
  let files = Router::new().fallback(move |uri: Uri, headers: HeaderMap| {
    let index = Arc::clone(&index);

    async move { serve_embedded::<E>(&uri, &headers, index.as_ref()) }
  });

  opts.apply(files)
}

fn serve_embedded<E: RustEmbed + 'static>(uri: &Uri, headers: &HeaderMap, index: &str) -> Response {
  let decoded = percent_decode_str(uri.path()).decode_utf8_lossy();
  let path = decoded.trim_start_matches('/');

  if path.is_empty() || path == index {
    return index_html::<E>(headers, index);
  }

  match E::get(path) {
    Some(file) => {
      let content_type = file.metadata.mimetype().to_owned();

      respond(file, &content_type, headers)
    }
    None => {
      if names_a_file(uri.path()) {
        return StatusCode::NOT_FOUND.into_response();
      }

      index_html::<E>(headers, index)
    }
  }
}

fn index_html<E: RustEmbed + 'static>(headers: &HeaderMap, index: &str) -> Response {
  match E::get(index) {
    Some(file) => respond(file, HTML, headers),
    None => StatusCode::NOT_FOUND.into_response(),
  }
}

fn respond(file: EmbeddedFile, content_type: &str, headers: &HeaderMap) -> Response {
  let etag = etag(file.metadata.sha256_hash());

  if is_unchanged(headers, &etag) {
    return (StatusCode::NOT_MODIFIED, [(header::ETAG, etag)]).into_response();
  }

  (
    [
      (header::CONTENT_TYPE, content_type.to_owned()),
      (header::ETAG, etag),
    ],
    file.data,
  )
    .into_response()
}

fn etag(hash: [u8; 32]) -> String {
  let hex: String = hash.iter().map(|byte| format!("{byte:02x}")).collect();

  format!("\"{hex}\"")
}

fn is_unchanged(headers: &HeaderMap, etag: &str) -> bool {
  headers
    .get_all(header::IF_NONE_MATCH)
    .iter()
    .filter_map(|value| value.to_str().ok())
    .flat_map(|value| value.split(','))
    .map(str::trim)
    .any(|candidate| candidate == "*" || candidate.trim_start_matches("W/") == etag)
}
