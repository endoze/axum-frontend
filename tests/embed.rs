use axum::body::Body;
use axum::http::{Request, StatusCode};
use tower::ServiceExt; // for oneshot

#[derive(axum_frontend::Embed)]
#[folder = "tests/fixtures/dist"]
struct TestAssets;

async fn body_string(resp: axum::response::Response) -> String {
  let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
    .await
    .expect("read body");

  String::from_utf8(bytes.to_vec()).expect("utf8")
}

#[tokio::test]
async fn serves_named_asset_with_mime() {
  let router = axum_frontend::embed_router::<TestAssets>();
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
  assert_eq!(
    resp.headers().get("content-type").unwrap(),
    "text/javascript"
  );

  let body = body_string(resp).await;

  assert!(body.contains("app-js"));
}

#[tokio::test]
async fn serves_index_at_root() {
  let router = axum_frontend::embed_router::<TestAssets>();
  let resp = router
    .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
    .await
    .unwrap();

  assert_eq!(resp.status(), StatusCode::OK);

  let body = body_string(resp).await;

  assert!(body.contains("id=\"root\""));
}

#[tokio::test]
async fn spa_fallback_for_client_route() {
  let router = axum_frontend::embed_router::<TestAssets>();
  let resp = router
    .oneshot(
      Request::builder()
        .uri("/some/client/route")
        .body(Body::empty())
        .unwrap(),
    )
    .await
    .unwrap();

  assert_eq!(resp.status(), StatusCode::OK);

  let body = body_string(resp).await;

  assert!(body.contains("id=\"root\""));
}

#[tokio::test]
async fn missing_asset_returns_404() {
  let router = axum_frontend::embed_router::<TestAssets>();
  let resp = router
    .oneshot(
      Request::builder()
        .uri("/missing.js")
        .body(Body::empty())
        .unwrap(),
    )
    .await
    .unwrap();

  assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn custom_index_served_for_client_route() {
  let router = axum_frontend::embed_router_with::<TestAssets>(
    axum_frontend::SpaOptions::new().index("app.html"),
  );
  let resp = router
    .oneshot(
      Request::builder()
        .uri("/deep/link")
        .body(Body::empty())
        .unwrap(),
    )
    .await
    .unwrap();

  assert_eq!(resp.status(), StatusCode::OK);

  let body = body_string(resp).await;

  assert!(body.contains("alt-root"));
}
