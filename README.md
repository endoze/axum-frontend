# Axum Frontend

![Build Status](https://github.com/endoze/axum-frontend/actions/workflows/ci.yml/badge.svg?branch=master)
[![Coverage Status](https://coveralls.io/repos/github/endoze/axum-frontend/badge.svg?branch=master)](https://coveralls.io/github/endoze/axum-frontend?branch=master)
[![Crate](https://img.shields.io/crates/v/axum-frontend.svg)](https://crates.io/crates/axum-frontend)
[![Docs](https://docs.rs/axum-frontend/badge.svg)](https://docs.rs/axum-frontend)

Serve a Vite (or any) frontend from axum: embed the built SPA in your binary, serve it from a directory on disk, or spawn and reverse-proxy the dev server.

## Installation

As a dependency of a Rust project:

```sh
cargo add axum-frontend
```

Dev mode (spawn the dev server and reverse-proxy to it) is behind the `dev` feature:

```sh
cargo add axum-frontend --features dev
```

## Library Usage

axum-frontend is provided as a crate that you can use in your own code.

Cargo.toml:
```toml
[dependencies]
axum = "0.8"
tokio = {version = "1", features = ["full"]}
axum-frontend = "0.1"
```

### Embed the built SPA in the binary

```rust,ignore
use axum::{routing::get, Router};
use axum_frontend::{embed_router, RustEmbed};

#[derive(RustEmbed)]
#[folder = "dist/"]
struct Assets;

let app = Router::new()
  .route("/api/health", get(|| async { "ok" }))
  // Your routes win; everything else is served from the embedded assets,
  // falling back to index.html for client-side routes.
  .merge(embed_router::<Assets>());
```

### Serve the built SPA from a directory

```rust,no_run
use axum::{routing::get, Router};
use axum_frontend::static_router;

let app = Router::new()
  .route("/api/health", get(|| async { "ok" }))
  .merge(static_router("dist"));
```

### Dev mode: spawn the dev server and reverse-proxy to it

Requires the `dev` feature.

```rust,ignore
use axum::{routing::get, Router};
use axum_frontend::DevConfig;

// Inside an async context:
// Spawns `pnpm dev` in ./frontend with --host/--port pinned, and waits until
// it accepts connections. The returned DevServer kills the dev server (and, on
// Unix, its whole process group) on drop.
let dev = DevConfig::vite()
  .cwd("frontend")
  .port(5173)
  .spawn()
  .await?;

let app = Router::new()
  .route("/api/health", get(|| async { "ok" }))
  // Proxy everything else to the dev server.
  .merge(dev.router(["/"]));
```

Use `DevConfig::new(["npm", "run", "dev"])` for a tool-neutral command, or
`dev_router(prefixes, target)` directly if you manage the dev server yourself.

## Inspiration

I wanted a single, ergonomic way to serve a Vite frontend from an axum backend:
one dev front door with hot-module reloading while developing, and a
self-contained binary (or a directory of assets) in production, without wiring up
the same proxy, embed, and SPA-fallback logic by hand in every project.
