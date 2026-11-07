//! The same request matrix against the embedded and on-disk sources.

use axum::body::Body;
use axum::http::{header, HeaderName, HeaderValue, Method, Request, StatusCode};
use axum::response::Response;
use axum::routing::get;
use axum::Router;
use axum_frontend::{
  embed_router, embed_router_with, static_router, static_router_with, SpaOptions,
};
use std::path::PathBuf;
use tower::ServiceExt;

const IMMUTABLE: &str = "public, max-age=31536000, immutable";

#[derive(axum_frontend::Embed)]
#[folder = "tests/fixtures/dist"]
struct TestAssets;

fn fixtures() -> PathBuf {
  PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/dist")
}

fn sources(opts: &SpaOptions) -> [(&'static str, Router); 2] {
  [
    ("embed", embed_router_with::<TestAssets>(opts.clone())),
    ("static", static_router_with(fixtures(), opts.clone())),
  ]
}

async fn send(router: Router, method: Method, path: &str) -> Response {
  let request = Request::builder()
    .method(method)
    .uri(path)
    .body(Body::empty())
    .unwrap();

  router.oneshot(request).await.unwrap()
}

async fn get_path(router: Router, path: &str) -> Response {
  send(router, Method::GET, path).await
}

async fn body_string(response: Response) -> String {
  let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
    .await
    .unwrap();

  String::from_utf8(bytes.to_vec()).unwrap()
}

fn cache_control(response: &Response) -> Option<&str> {
  response
    .headers()
    .get(header::CACHE_CONTROL)
    .map(|value| value.to_str().unwrap())
}

#[tokio::test]
async fn writes_never_reach_the_spa() {
  for (source, router) in sources(&SpaOptions::new()) {
    for path in ["/client/route", "/app.js", "/"] {
      let response = send(router.clone(), Method::POST, path).await;

      assert_eq!(
        response.status(),
        StatusCode::METHOD_NOT_ALLOWED,
        "{source} {path}"
      );
      assert_eq!(response.headers()[header::ALLOW], "GET,HEAD");
      assert_eq!(body_string(response).await, "", "{source} {path}");
    }
  }
}

#[tokio::test]
async fn reads_reach_the_spa() {
  for (source, router) in sources(&SpaOptions::new()) {
    for method in [Method::GET, Method::HEAD] {
      let response = send(router.clone(), method.clone(), "/client/route").await;

      assert_eq!(response.status(), StatusCode::OK, "{source} {method}");
    }
  }
}

#[tokio::test]
async fn excluded_paths_are_404_for_every_method() {
  let opts = SpaOptions::new().exclude(["/api"]);

  for (source, router) in sources(&opts) {
    for path in ["/api", "/api/missing", "/api/v1/users.json"] {
      for method in [Method::GET, Method::POST, Method::DELETE] {
        let response = send(router.clone(), method.clone(), path).await;

        assert_eq!(
          response.status(),
          StatusCode::NOT_FOUND,
          "{source} {method} {path}"
        );
        assert_eq!(cache_control(&response), None, "{source} {path}");
      }
    }
  }
}

#[tokio::test]
async fn an_excluded_prefix_stops_at_a_path_segment() {
  let opts = SpaOptions::new().exclude(["/api"]);

  for (source, router) in sources(&opts) {
    let response = get_path(router, "/apiary").await;

    assert_eq!(response.status(), StatusCode::OK, "{source}");
    assert!(body_string(response).await.contains("id=\"root\""));
  }
}

#[tokio::test]
async fn a_trailing_slash_excludes_only_below_the_path() {
  let opts = SpaOptions::new().exclude(["/admin/"]);

  for (source, router) in sources(&opts) {
    let bare = get_path(router.clone(), "/admin").await;
    let below = get_path(router, "/admin/missing").await;

    assert_eq!(bare.status(), StatusCode::OK, "{source}");
    assert_eq!(below.status(), StatusCode::NOT_FOUND, "{source}");
  }
}

#[tokio::test]
async fn exclusion_wins_over_a_real_file() {
  let opts = SpaOptions::new().exclude(["/assets"]);

  for (source, router) in sources(&opts) {
    let response = get_path(router, "/assets/app-abc123.js").await;

    assert_eq!(response.status(), StatusCode::NOT_FOUND, "{source}");
  }
}

#[tokio::test]
async fn a_missing_path_with_a_dot_is_404() {
  for (source, router) in sources(&SpaOptions::new()) {
    for path in ["/missing.js", "/assets/missing.js", "/users/john.doe"] {
      let response = get_path(router.clone(), path).await;

      assert_eq!(response.status(), StatusCode::NOT_FOUND, "{source} {path}");
      assert_eq!(cache_control(&response), None, "{source} {path}");
    }
  }
}

#[tokio::test]
async fn exclusion_sees_through_encoding_and_path_tricks() {
  let opts = SpaOptions::new().exclude(["/api"]);
  let paths = [
    "/%61pi/missing",
    "/api%2Fmissing",
    "//api/missing",
    "///api",
    "/./api/missing",
    "/x/../api/missing",
  ];

  for (source, router) in sources(&opts) {
    for path in paths {
      for method in [Method::GET, Method::POST] {
        let response = send(router.clone(), method.clone(), path).await;

        assert_eq!(
          response.status(),
          StatusCode::NOT_FOUND,
          "{source} {method} {path}"
        );
      }
    }
  }
}

#[tokio::test]
async fn an_excluded_file_stays_hidden_behind_an_encoded_path() {
  let opts = SpaOptions::new().exclude(["/assets"]);

  for (source, router) in sources(&opts) {
    for path in ["/%61ssets/app-abc123.js", "//assets/app-abc123.js"] {
      let response = get_path(router.clone(), path).await;

      assert_eq!(response.status(), StatusCode::NOT_FOUND, "{source} {path}");
    }
  }
}

#[tokio::test]
async fn a_backslash_does_not_get_past_an_exclusion() {
  let opts = SpaOptions::new().exclude(["/assets"]);

  for (source, router) in sources(&opts) {
    for path in ["/assets%5Capp-abc123.js", "/assets\\app-abc123.js"] {
      let response = get_path(router.clone(), path).await;

      assert_eq!(response.status(), StatusCode::NOT_FOUND, "{source} {path}");
    }
  }
}

#[tokio::test]
async fn an_immutable_prefix_reads_a_backslash_as_a_separator() {
  let opts = SpaOptions::new().immutable(["/assets/"]);
  let router = embed_router_with::<TestAssets>(opts);

  for path in ["/assets%5Capp-abc123.js", "/assets\\app-abc123.js"] {
    let response = get_path(router.clone(), path).await;

    assert_eq!(response.status(), StatusCode::OK, "{path}");
    assert_eq!(cache_control(&response), Some(IMMUTABLE), "{path}");
  }
}

#[tokio::test]
async fn an_immutable_prefix_sees_through_encoding() {
  let opts = SpaOptions::new().immutable(["/assets/"]);

  for (source, router) in sources(&opts) {
    let response = get_path(router, "/%61ssets/app-abc123.js").await;

    assert_eq!(response.status(), StatusCode::OK, "{source}");
    assert_eq!(cache_control(&response), Some(IMMUTABLE), "{source}");
  }
}

#[tokio::test]
async fn an_encoded_filename_is_served() {
  for (source, router) in sources(&SpaOptions::new()) {
    let response = get_path(router, "/my%20file.txt").await;

    assert_eq!(response.status(), StatusCode::OK, "{source}");
    assert_eq!(body_string(response).await, "plain text\n", "{source}");
  }
}

#[tokio::test]
async fn a_missing_path_with_an_encoded_dot_is_404() {
  for (source, router) in sources(&SpaOptions::new()) {
    let response = get_path(router, "/missing%2Ejs").await;

    assert_eq!(response.status(), StatusCode::NOT_FOUND, "{source}");
  }
}

#[tokio::test]
async fn a_path_that_is_not_utf8_gets_the_same_answer_from_both_sources() {
  let [(_, embed), (_, on_disk)] = sources(&SpaOptions::new());
  let embed = get_path(embed, "/%ff").await;
  let on_disk = get_path(on_disk, "/%ff").await;

  assert_eq!(embed.status(), on_disk.status());
}

#[tokio::test]
async fn nesting_answers_a_trailing_slash_before_the_router_is_reached() {
  for (source, router) in sources(&SpaOptions::new()) {
    let app = Router::new().nest("/app", router);
    let bare = get_path(app.clone(), "/app").await;
    let slash = get_path(app, "/app/").await;

    assert_eq!(bare.status(), StatusCode::OK, "{source}");
    assert_eq!(slash.status(), StatusCode::NOT_FOUND, "{source}");
  }

  let plain: Router = Router::new().nest("/app", Router::new().fallback(|| async { "plain" }));
  let response = get_path(plain, "/app/").await;

  assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn a_custom_index_is_served_at_the_root_and_for_client_routes() {
  let opts = SpaOptions::new().index("app.html");

  for (source, router) in sources(&opts) {
    for path in ["/", "/client/route"] {
      let response = get_path(router.clone(), path).await;

      assert_eq!(response.status(), StatusCode::OK, "{source} {path}");
      assert!(
        body_string(response).await.contains("alt-root"),
        "{source} {path}"
      );
    }
  }
}

#[tokio::test]
async fn a_directory_path_is_a_client_route() {
  for (source, router) in sources(&SpaOptions::new()) {
    for path in ["/assets", "/assets/"] {
      let response = get_path(router.clone(), path).await;

      assert_eq!(response.status(), StatusCode::OK, "{source} {path}");
      assert!(
        body_string(response).await.contains("id=\"root\""),
        "{source} {path}"
      );
    }
  }
}

#[tokio::test]
async fn html_is_revalidated() {
  for (source, router) in sources(&SpaOptions::new()) {
    for path in ["/", "/client/route", "/index.html"] {
      let response = get_path(router.clone(), path).await;

      assert_eq!(response.status(), StatusCode::OK, "{source} {path}");
      assert_eq!(
        cache_control(&response),
        Some("no-cache"),
        "{source} {path}"
      );
    }
  }
}

#[tokio::test]
async fn files_are_revalidated_unless_named_immutable() {
  for (source, router) in sources(&SpaOptions::new()) {
    let response = get_path(router, "/assets/app-abc123.js").await;

    assert_eq!(cache_control(&response), Some("no-cache"), "{source}");
  }
}

#[tokio::test]
async fn hashed_assets_are_immutable_when_named() {
  let opts = SpaOptions::new().immutable(["/assets/"]);

  for (source, router) in sources(&opts) {
    let hashed = get_path(router.clone(), "/assets/app-abc123.js").await;
    let unhashed = get_path(router, "/app.js").await;

    assert_eq!(cache_control(&hashed), Some(IMMUTABLE), "{source}");
    assert_eq!(cache_control(&unhashed), Some("no-cache"), "{source}");
  }
}

#[tokio::test]
async fn html_under_an_immutable_prefix_is_still_revalidated() {
  let opts = SpaOptions::new().immutable(["/assets/"]);

  for (source, router) in sources(&opts) {
    let response = get_path(router, "/assets/client-route").await;

    assert_eq!(response.status(), StatusCode::OK, "{source}");
    assert_eq!(cache_control(&response), Some("no-cache"), "{source}");
  }
}

/// A browser's second request, carrying whichever validator the first response
/// offered: an embedded file has an `ETag`, a file on disk a `Last-Modified`.
fn validator(response: &Response) -> (HeaderName, HeaderValue) {
  let headers = response.headers();

  match headers.get(header::ETAG) {
    Some(etag) => (header::IF_NONE_MATCH, etag.clone()),
    None => (
      header::IF_MODIFIED_SINCE,
      headers[header::LAST_MODIFIED].clone(),
    ),
  }
}

#[tokio::test]
async fn an_unchanged_file_answers_304_without_touching_the_stored_policy() {
  let opts = SpaOptions::new().immutable(["/assets/"]);

  for (source, router) in sources(&opts) {
    for path in [
      "/",
      "/app.js",
      "/assets/app-abc123.js",
      "/assets/client-route",
    ] {
      let first = get_path(router.clone(), path).await;
      let (name, value) = validator(&first);
      let request = Request::get(path)
        .header(name, value)
        .body(Body::empty())
        .unwrap();
      let second = router.clone().oneshot(request).await.unwrap();

      assert_eq!(second.status(), StatusCode::NOT_MODIFIED, "{source} {path}");
      assert_eq!(cache_control(&second), None, "{source} {path}");
      assert_eq!(body_string(second).await, "", "{source} {path}");
    }
  }
}

#[tokio::test]
async fn a_policy_the_source_set_is_kept() {
  let files: Router = Router::new().fallback(|| async {
    (
      [(header::CACHE_CONTROL, "max-age=60")],
      "from the serving mode",
    )
  });
  let router = SpaOptions::new().immutable(["/assets/"]).apply(files);

  for path in ["/client/route", "/assets/app.js"] {
    let response = get_path(router.clone(), path).await;

    assert_eq!(cache_control(&response), Some("max-age=60"), "{path}");
  }
}

#[tokio::test]
async fn an_applied_policy_upgrades_a_routers_default() {
  let opts = SpaOptions::new().exclude(["/api"]).immutable(["/assets/"]);
  let embed = opts.clone().apply(embed_router::<TestAssets>());
  let on_disk = opts.apply(static_router(fixtures()));

  for (source, router) in [("embed", embed), ("static", on_disk)] {
    let hashed = get_path(router.clone(), "/assets/app-abc123.js").await;
    let page = get_path(router.clone(), "/client/route").await;
    let miss = get_path(router, "/api/missing").await;

    assert_eq!(cache_control(&hashed), Some(IMMUTABLE), "{source}");
    assert_eq!(cache_control(&page), Some("no-cache"), "{source}");
    assert_eq!(miss.status(), StatusCode::NOT_FOUND, "{source}");
  }
}

#[tokio::test]
async fn an_outer_default_keeps_an_inner_immutable_policy() {
  let opts = SpaOptions::new().immutable(["/assets/"]);

  for (source, router) in sources(&opts) {
    let router = SpaOptions::new().apply(router);
    let response = get_path(router, "/assets/app-abc123.js").await;

    assert_eq!(cache_control(&response), Some(IMMUTABLE), "{source}");
  }
}

#[tokio::test]
async fn the_apps_routes_win_after_a_merge() {
  let opts = SpaOptions::new().exclude(["/api"]);

  for (source, router) in sources(&opts) {
    let app = Router::new()
      .route("/api/health", get(|| async { "ok" }))
      .route("/api/submit", axum::routing::post(|| async { "done" }))
      .merge(router);
    let health = get_path(app.clone(), "/api/health").await;
    let submit = send(app, Method::POST, "/api/submit").await;

    assert_eq!(health.status(), StatusCode::OK, "{source}");
    assert_eq!(cache_control(&health), None, "{source}");
    assert_eq!(submit.status(), StatusCode::OK, "{source}");
  }
}

#[tokio::test]
async fn prefixes_are_relative_to_a_nested_mount() {
  let opts = SpaOptions::new().immutable(["/assets/"]);

  for (source, router) in sources(&opts) {
    let app = Router::new().nest("/mount", router);
    let response = get_path(app, "/mount/assets/app-abc123.js").await;

    assert_eq!(response.status(), StatusCode::OK, "{source}");
    assert_eq!(cache_control(&response), Some(IMMUTABLE), "{source}");
  }
}

#[test]
#[should_panic(expected = "must start with `/`")]
fn an_excluded_prefix_without_a_leading_slash_panics() {
  let _ = SpaOptions::new().exclude(["api"]);
}

#[test]
#[should_panic(expected = "must start with `/`")]
fn an_immutable_prefix_without_a_leading_slash_panics() {
  let _ = SpaOptions::new().immutable(["assets/"]);
}
