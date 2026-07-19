use axum::body::Body;
use axum::http::{Request, StatusCode};
use std::path::PathBuf;
use tower::ServiceExt;

fn fixtures() -> PathBuf {
  PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/dist")
}

#[tokio::test]
async fn serves_named_file_from_dir() {
  let router = axum_frontend::static_router(fixtures());
  let resp = router
    .oneshot(
      Request::builder()
        .uri("/app.js")
        .body(Body::empty())
        .unwrap(),
    )
    .await
    .unwrap();

  assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn spa_fallback_serves_index() {
  let router = axum_frontend::static_router(fixtures());
  let resp = router
    .oneshot(
      Request::builder()
        .uri("/client/route")
        .body(Body::empty())
        .unwrap(),
    )
    .await
    .unwrap();

  assert_eq!(resp.status(), StatusCode::OK);

  let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
    .await
    .unwrap();
  let body = String::from_utf8(bytes.to_vec()).unwrap();

  assert!(body.contains("id=\"root\""));
}

#[tokio::test]
async fn missing_asset_falls_back_to_index() {
  // static_router serves the index document (200) for ANY unresolved path,
  // including a missing file with an extension — unlike embed_router, which 404s.
  let router = axum_frontend::static_router(fixtures());
  let resp = router
    .oneshot(
      Request::builder()
        .uri("/missing.js")
        .body(Body::empty())
        .unwrap(),
    )
    .await
    .unwrap();

  assert_eq!(resp.status(), StatusCode::OK);

  let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
    .await
    .unwrap();
  let body = String::from_utf8(bytes.to_vec()).unwrap();

  assert!(body.contains("id=\"root\""));
}

#[tokio::test]
async fn custom_index_fallback_serves_named_document() {
  let router = axum_frontend::static_router_with(
    fixtures(),
    axum_frontend::SpaOptions::new().index("app.html"),
  );
  let resp = router
    .oneshot(
      Request::builder()
        .uri("/client/route")
        .body(Body::empty())
        .unwrap(),
    )
    .await
    .unwrap();

  assert_eq!(resp.status(), StatusCode::OK);

  let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
    .await
    .unwrap();
  let body = String::from_utf8(bytes.to_vec()).unwrap();

  assert!(body.contains("alt-root"));
}
