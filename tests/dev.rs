#![cfg(feature = "dev")]

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::routing::get;
use axum::Router;
use std::time::Duration;
use tower::ServiceExt;

#[tokio::test]
async fn wait_until_ready_succeeds_when_port_is_open() {
  let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
  let port = listener.local_addr().unwrap().port();

  axum_frontend::wait_until_ready("127.0.0.1", port, Duration::from_secs(2))
    .await
    .expect("port is open, should be ready");
}

#[tokio::test]
async fn wait_until_ready_times_out_on_closed_port() {
  // Bind then drop to obtain a very likely-closed port number.
  let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
  let port = listener.local_addr().unwrap().port();

  drop(listener);

  let result = axum_frontend::wait_until_ready("127.0.0.1", port, Duration::from_millis(300)).await;

  assert!(result.is_err(), "closed port should time out");
}

#[tokio::test]
async fn dev_router_proxies_to_upstream() {
  // Start a stub "vite" upstream.
  let upstream = Router::new().route("/hello", get(|| async { "from-upstream" }));
  let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
  let port = listener.local_addr().unwrap().port();

  tokio::spawn(async move {
    axum::serve(listener, upstream).await.unwrap();
  });

  axum_frontend::wait_until_ready("127.0.0.1", port, Duration::from_secs(2))
    .await
    .unwrap();

  let proxy = axum_frontend::dev_router(["/"], &format!("http://127.0.0.1:{port}"));
  let resp = proxy
    .oneshot(
      Request::builder()
        .uri("/hello")
        .body(Body::empty())
        .unwrap(),
    )
    .await
    .unwrap();

  assert_eq!(resp.status(), StatusCode::OK);

  let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
    .await
    .unwrap();

  assert_eq!(&bytes[..], b"from-upstream");
}

#[tokio::test]
async fn spawn_reports_error_for_missing_binary() {
  let result = axum_frontend::DevConfig::new(["definitely-not-a-real-binary-xyz"])
    .spawn()
    .await;

  assert!(result.is_err(), "spawning a missing binary should error");
}

#[tokio::test]
async fn dev_router_preserves_path_for_prefixes_and_narrows_scope() {
  let upstream = Router::new()
    .route("/a/hello", get(|| async { "a-hello" }))
    .route("/b/hello", get(|| async { "b-hello" }));
  let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
  let port = listener.local_addr().unwrap().port();

  tokio::spawn(async move {
    axum::serve(listener, upstream).await.unwrap();
  });

  axum_frontend::wait_until_ready("127.0.0.1", port, Duration::from_secs(2))
    .await
    .unwrap();

  let target = format!("http://127.0.0.1:{port}");
  let router = axum_frontend::dev_router(["/a", "/b"], &target);

  // Prefix is preserved end-to-end: /a/hello proxies as /a/hello (not /hello).
  let resp = router
    .clone()
    .oneshot(
      Request::builder()
        .uri("/a/hello")
        .body(Body::empty())
        .unwrap(),
    )
    .await
    .unwrap();

  assert_eq!(resp.status(), StatusCode::OK);

  let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
    .await
    .unwrap();

  assert_eq!(&bytes[..], b"a-hello");

  // A path outside the configured prefixes is NOT proxied.
  let resp = router
    .oneshot(
      Request::builder()
        .uri("/c/nope")
        .body(Body::empty())
        .unwrap(),
    )
    .await
    .unwrap();

  assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn dev_router_empty_prefixes_proxies_everything() {
  let upstream = Router::new().route("/anything", get(|| async { "ok" }));
  let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
  let port = listener.local_addr().unwrap().port();

  tokio::spawn(async move {
    axum::serve(listener, upstream).await.unwrap();
  });

  axum_frontend::wait_until_ready("127.0.0.1", port, Duration::from_secs(2))
    .await
    .unwrap();

  let target = format!("http://127.0.0.1:{port}");
  let prefixes: Vec<&str> = vec![];
  let router = axum_frontend::dev_router(prefixes, &target);
  let resp = router
    .oneshot(
      Request::builder()
        .uri("/anything")
        .body(Body::empty())
        .unwrap(),
    )
    .await
    .unwrap();

  assert_eq!(resp.status(), StatusCode::OK);
}

#[test]
#[should_panic]
fn dev_router_panics_on_prefix_without_leading_slash() {
  // axum's route_service rejects paths that don't start with `/`.
  let _ = axum_frontend::dev_router(["api"], "http://127.0.0.1:5173");
}

#[test]
#[should_panic]
fn dev_router_panics_on_overlapping_prefixes() {
  // Registering the same non-`/` prefix twice builds overlapping routes.
  let _ = axum_frontend::dev_router(["/api", "/api"], "http://127.0.0.1:5173");
}
