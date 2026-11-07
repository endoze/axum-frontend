//! Shared options and the route and cache policy for every serving mode.

#![deny(missing_docs)]

use axum::extract::{Request, State};
use axum::http::{header, HeaderValue, Method, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::Router;
use percent_encoding::percent_decode_str;
use std::sync::Arc;

const REVALIDATE: &str = "no-cache";
const IMMUTABLE: &str = "public, max-age=31536000, immutable";
const SEPARATORS: [char; 2] = ['/', '\\'];

/// Options shared by the SPA routers, and the policy [`SpaOptions::apply`] puts
/// in front of any of them.
#[derive(Debug, Clone)]
pub struct SpaOptions {
  pub(crate) index: String,
  excluded: Vec<String>,
  immutable: Vec<String>,
}

impl SpaOptions {
  /// Defaults: index document `index.html`, no excluded paths, and no immutable
  /// paths.
  #[must_use]
  pub fn new() -> Self {
    Self {
      index: "index.html".to_string(),
      excluded: Vec::new(),
      immutable: Vec::new(),
    }
  }

  /// Set the SPA entry document served for `/` and unmatched client-side routes.
  #[must_use]
  pub fn index(mut self, name: impl Into<String>) -> Self {
    self.index = name.into();

    self
  }

  /// Answer `404` for every method under these path prefixes, so a missed API
  /// route never becomes the SPA. Repeated calls add to the list.
  ///
  /// `/api` matches `/api` and `/api/users` but not `/apiary`, and `/api/`
  /// matches only below `/api`. Prefixes are relative to where the router is
  /// mounted, so a nested router sees paths with the nest prefix removed.
  ///
  /// Matching uses the decoded path with `.`, `..`, and repeated slashes resolved,
  /// so `/%61pi/users`, `//api/users`, and `/api\users` count as `/api/users`.
  ///
  /// Matching is case-sensitive. On a case-insensitive filesystem, folder mode and
  /// debug embedded builds can still serve `/API/users.json` with `/api` excluded,
  /// so do not use this to keep files private.
  ///
  /// ```
  /// let opts = axum_frontend::SpaOptions::new().exclude(["/api", "/auth/"]);
  /// ```
  ///
  /// # Panics
  ///
  /// Panics if a prefix does not start with `/`, since it could never match.
  #[must_use]
  pub fn exclude<I, S>(mut self, prefixes: I) -> Self
  where
    I: IntoIterator<Item = S>,
    S: Into<String>,
  {
    self.excluded.extend(checked_prefixes(prefixes));

    self
  }

  /// Let browsers keep successful non-HTML responses under these path prefixes
  /// for a year without asking again. Repeated calls add to the list, and
  /// prefixes match as in [`SpaOptions::exclude`].
  ///
  /// Name only folders of content-hashed files: an unhashed file here stays
  /// stale in browsers after it changes. Leave this off in dev mode, where files
  /// change under the same name.
  ///
  /// ```
  /// let opts = axum_frontend::SpaOptions::new().immutable(["/assets/"]);
  /// ```
  ///
  /// # Panics
  ///
  /// Panics if a prefix does not start with `/`, since it could never match.
  #[must_use]
  pub fn immutable<I, S>(mut self, prefixes: I) -> Self
  where
    I: IntoIterator<Item = S>,
    S: Into<String>,
  {
    self.immutable.extend(checked_prefixes(prefixes));

    self
  }

  /// Put this policy in front of `files`, which can come from any serving mode.
  ///
  /// Excluded paths answer `404`, and any other non-read answers `405`. Successful
  /// responses get `Cache-Control: no-cache`, or the immutable policy under
  /// [`SpaOptions::immutable`], unless `files` already set a policy of its own.
  ///
  /// The SPA routers already apply their options, so this is for dev mode, and
  /// for placing the policy on the far side of a layer such as authentication.
  /// An applied policy can still upgrade the routers' default `no-cache` to
  /// immutable.
  ///
  /// ```
  /// use axum_frontend::{embed_router, Embed, SpaOptions};
  ///
  /// #[derive(Embed)]
  /// #[folder = "tests/fixtures/dist"]
  /// struct Assets;
  ///
  /// let app = SpaOptions::new()
  ///   .exclude(["/api"])
  ///   .apply(embed_router::<Assets>());
  /// ```
  pub fn apply(self, files: Router) -> Router {
    files.layer(middleware::from_fn_with_state(Arc::new(self), enforce))
  }
}

impl Default for SpaOptions {
  fn default() -> Self {
    Self::new()
  }
}

/// Marks a response whose `Cache-Control` is this policy's default rather than
/// a choice made by the serving mode, so an outer policy may still upgrade it.
#[derive(Clone, Copy)]
struct DefaultCachePolicy;

async fn enforce(State(opts): State<Arc<SpaOptions>>, request: Request, next: Next) -> Response {
  let path = normalized(request.uri().path());

  if is_under_any(&path, &opts.excluded) {
    return StatusCode::NOT_FOUND.into_response();
  }

  if !matches!(*request.method(), Method::GET | Method::HEAD) {
    return (
      StatusCode::METHOD_NOT_ALLOWED,
      [(header::ALLOW, "GET,HEAD")],
    )
      .into_response();
  }

  let immutable = is_under_any(&path, &opts.immutable);
  let mut response = next.run(request).await;

  set_cache_policy(&mut response, immutable);

  response
}

/// A `304` is left alone on purpose: it has no `Content-Type` to rule out HTML,
/// and a cache keeps the stored policy when a `304` carries none.
fn set_cache_policy(response: &mut Response, immutable: bool) {
  if !response.status().is_success() {
    return;
  }

  let is_default = response.extensions().get::<DefaultCachePolicy>().is_some();
  let is_set = response.headers().contains_key(header::CACHE_CONTROL);

  if is_set && !(immutable && is_default) {
    return;
  }

  let is_html = response
    .headers()
    .get(header::CONTENT_TYPE)
    .and_then(|value| value.to_str().ok())
    .is_some_and(|value| value.starts_with("text/html"));
  let policy = if immutable && !is_html {
    IMMUTABLE
  } else {
    response.extensions_mut().insert(DefaultCachePolicy);

    REVALIDATE
  };

  response
    .headers_mut()
    .insert(header::CACHE_CONTROL, HeaderValue::from_static(policy));
}

/// Whether a path with nothing behind it names a file rather than a client-side
/// route, so that a miss is a `404` instead of the index document.
pub(crate) fn names_a_file(path: &str) -> bool {
  percent_decode_str(path).any(|byte| byte == b'.')
}

/// `path` as a file server would resolve it: percent-decoded once, with `\` read
/// as `/`, and with `.`, `..`, and repeated slashes gone. Matching the raw path
/// would let `/%61pi` or `//api` past a prefix while the source behind it still
/// serves the same file.
fn normalized(path: &str) -> String {
  let decoded = percent_decode_str(path).decode_utf8_lossy();
  let mut segments: Vec<&str> = Vec::new();

  for segment in decoded.split(SEPARATORS) {
    match segment {
      "" | "." => {}
      ".." => {
        segments.pop();
      }
      segment => segments.push(segment),
    }
  }

  let mut normal = format!("/{}", segments.join("/"));

  if decoded.ends_with(SEPARATORS) && !segments.is_empty() {
    normal.push('/');
  }

  normal
}

fn is_under_any(path: &str, prefixes: &[String]) -> bool {
  prefixes.iter().any(|prefix| is_under(path, prefix))
}

fn is_under(path: &str, prefix: &str) -> bool {
  if prefix.ends_with('/') {
    return path.starts_with(prefix);
  }

  path
    .strip_prefix(prefix)
    .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}

fn checked_prefixes<I, S>(prefixes: I) -> Vec<String>
where
  I: IntoIterator<Item = S>,
  S: Into<String>,
{
  let prefixes: Vec<String> = prefixes.into_iter().map(Into::into).collect();

  for prefix in &prefixes {
    assert!(
      prefix.starts_with('/'),
      "path prefix {prefix:?} must start with `/`"
    );
  }

  prefixes
}
