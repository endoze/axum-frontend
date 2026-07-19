//! Dev mode: spawn & own a frontend dev server and reverse-proxy to it.

#![deny(missing_docs)]

use axum::Router;
use axum_reverse_proxy::ReverseProxy;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;
use tokio::process::{Child, Command};

/// Proxy one or more path prefixes to `target`, preserving the full forwarded
/// path. `"/"` proxies everything (mounted as a fallback); a non-`"/"` prefix
/// proxies that subtree, mounted so the prefix is *not* stripped. Merge the
/// result into your app: the `"/"` catch-all is a fallback, so your explicit
/// routes take precedence; non-`"/"` prefixes are real routes and will conflict
/// (panic on merge) with an app route of the same path.
///
/// An empty iterator is treated as `["/"]`. At most one `"/"` catch-all is
/// honored (a second is ignored to avoid a double-fallback panic on merge).
///
/// # Panics
///
/// Panics at router-build time if a prefix does not start with `/`, or if two
/// non-`"/"` prefixes overlap (including exact duplicates) — axum rejects routes
/// without a leading slash and rejects overlapping route paths.
pub fn dev_router<I, S>(prefixes: I, target: &str) -> Router
where
  I: IntoIterator<Item = S>,
  S: Into<String>,
{
  let mut prefixes: Vec<String> = prefixes.into_iter().map(Into::into).collect();

  if prefixes.is_empty() {
    prefixes.push("/".to_string());
  }

  let mut router = Router::new();
  let mut has_catch_all = false;

  for prefix in prefixes {
    let proxy = ReverseProxy::new("/", target);

    if prefix == "/" {
      if has_catch_all {
        continue;
      }

      has_catch_all = true;
      router = router.fallback_service(proxy);
    } else {
      let base = prefix.trim_end_matches('/').to_string();
      let wildcard = format!("{base}/{{*rest}}");

      router = router
        .route_service(&base, proxy.clone())
        .route_service(&wildcard, proxy);
    }
  }

  router
}

/// Poll `host:port` until it accepts a TCP connection or `timeout` elapses.
///
/// Readiness here means the port accepts a **TCP connection**, not that the dev
/// server is ready to serve HTTP: some tools (Vite included) accept connections
/// before finishing dependency optimization, so the very first proxied request
/// may still briefly fail.
///
/// # Errors
///
/// Returns an [`std::io::ErrorKind::TimedOut`] error if `host:port` does not
/// accept a connection within `timeout`.
pub async fn wait_until_ready(host: &str, port: u16, timeout: Duration) -> std::io::Result<()> {
  let addr = format!("{host}:{port}");
  let start = tokio::time::Instant::now();

  loop {
    match tokio::net::TcpStream::connect(&addr).await {
      Ok(_) => return Ok(()),
      Err(e) => {
        if start.elapsed() >= timeout {
          return Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            format!("dev server on {addr} not ready: {e}"),
          ));
        }

        tokio::time::sleep(Duration::from_millis(100)).await;
      }
    }
  }
}

/// Whether to inject a dev tool's host/port flags into the spawn command.
#[derive(Debug, Clone, Copy)]
enum FlagStyle {
  /// No flags appended (tool-neutral).
  None,
  /// Vite: append `--host <host> --port <port> --strictPort`.
  Vite,
}

/// Configuration for spawning a frontend dev server.
#[derive(Debug, Clone)]
pub struct DevConfig {
  command: Vec<String>,
  cwd: Option<PathBuf>,
  env: HashMap<String, String>,
  host: String,
  port: u16,
  ready_timeout: Duration,
  flags: FlagStyle,
}

impl DevConfig {
  /// Tool-neutral config for the given command (program + args). No dev-tool
  /// flags are injected; include any flags you need in `command` yourself.
  ///
  /// Defaults: host `127.0.0.1`, port `5173`, readiness timeout 60s.
  #[must_use]
  pub fn new<I, S>(command: I) -> Self
  where
    I: IntoIterator<Item = S>,
    S: Into<String>,
  {
    Self {
      command: command.into_iter().map(Into::into).collect(),
      cwd: None,
      env: HashMap::new(),
      host: "127.0.0.1".to_string(),
      port: 5173,
      ready_timeout: Duration::from_secs(60),
      flags: FlagStyle::None,
    }
  }

  /// Preset for Vite: runs `pnpm dev` and injects `--host <host> --port <port>
  /// --strictPort` at spawn time.
  ///
  /// Vite defaults to binding `localhost`, which on many systems resolves to
  /// IPv6 `::1` only. The readiness probe and the reverse-proxy target both use
  /// the configured `host` (default IPv4 `127.0.0.1`), so pinning `--host` keeps
  /// them in agreement — otherwise nothing can reach the dev server, the
  /// readiness wait times out, and the server exits before it binds.
  #[must_use]
  pub fn vite() -> Self {
    let mut config = Self::new(["pnpm", "dev"]);

    config.flags = FlagStyle::Vite;

    config
  }

  /// Override the command (program + args), e.g. `["pnpm", "dev"]`.
  #[must_use]
  pub fn command<I, S>(mut self, parts: I) -> Self
  where
    I: IntoIterator<Item = S>,
    S: Into<String>,
  {
    self.command = parts.into_iter().map(Into::into).collect();

    self
  }

  /// Working directory to run the command in (e.g. `"frontend"`).
  #[must_use]
  pub fn cwd(mut self, dir: impl Into<PathBuf>) -> Self {
    self.cwd = Some(dir.into());

    self
  }

  /// Add an environment variable for the child process.
  #[must_use]
  pub fn env(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
    self.env.insert(key.into(), value.into());

    self
  }

  /// Set the host the dev server binds, and the readiness-probe / proxy target.
  #[must_use]
  pub fn host(mut self, host: impl Into<String>) -> Self {
    self.host = host.into();

    self
  }

  /// Set the port the dev server listens on.
  #[must_use]
  pub fn port(mut self, port: u16) -> Self {
    self.port = port;

    self
  }

  /// How long `spawn` waits for the dev server to accept connections.
  #[must_use]
  pub fn ready_timeout(mut self, timeout: Duration) -> Self {
    self.ready_timeout = timeout;

    self
  }

  /// Full argv for the dev command (program + args), including any injected
  /// dev-tool flags (see [`DevConfig::vite`]).
  fn command_line(&self) -> Vec<String> {
    let mut argv = self.command.clone();

    if let FlagStyle::Vite = self.flags {
      argv.push("--host".to_string());
      argv.push(self.host.clone());
      argv.push("--port".to_string());
      argv.push(self.port.to_string());
      argv.push("--strictPort".to_string());
    }

    argv
  }

  /// Spawn the dev server and wait until it accepts connections.
  ///
  /// The returned [`DevServer`] owns the child process and kills it on drop. On
  /// Unix the child runs in its own process group, so grandchildren (such as the
  /// real Vite server spawned by `pnpm dev`) are reaped too rather than orphaned.
  ///
  /// # Errors
  ///
  /// Returns an error if the command is empty
  /// ([`std::io::ErrorKind::InvalidInput`]), if the process fails to spawn (for
  /// example, the program is not found), or if the dev server does not accept
  /// connections before the readiness timeout (see [`DevConfig::ready_timeout`]).
  pub async fn spawn(self) -> std::io::Result<DevServer> {
    let argv = self.command_line();
    let (program, args) = argv
      .split_first()
      .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "empty command"))?;

    let mut builder = std::process::Command::new(program);

    builder.args(args);

    if let Some(dir) = &self.cwd {
      builder.current_dir(dir);
    }

    for (k, v) in &self.env {
      builder.env(k, v);
    }

    // On Unix, run the dev server in its own process group so drop can reap the
    // whole tree: `pnpm dev` runs the real Vite server as a grandchild, and
    // killing only the direct child would orphan it (leaking the port).
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut builder, 0);

    let mut cmd = Command::from(builder);

    cmd.kill_on_drop(true);

    let child = cmd.spawn()?;

    #[cfg(unix)]
    let pgid = child.id().and_then(|id| i32::try_from(id).ok());

    if let Err(e) = wait_until_ready(&self.host, self.port, self.ready_timeout).await {
      // Readiness failed: reap the whole group before surfacing the error
      // (`kill_on_drop` alone would only get the direct child).
      #[cfg(unix)]
      if let Some(pgid) = pgid {
        kill_process_group(pgid);
      }

      return Err(e);
    }

    Ok(DevServer {
      child,
      host: self.host,
      port: self.port,
      #[cfg(unix)]
      pgid,
    })
  }
}

/// Owns the spawned dev server; kills it (and, on Unix, its whole process
/// group) when dropped.
#[derive(Debug)]
pub struct DevServer {
  child: Child,
  host: String,
  port: u16,
  #[cfg(unix)]
  pgid: Option<i32>,
}

impl Drop for DevServer {
  fn drop(&mut self) {
    // Reap the whole process group so grandchildren (e.g. the real Vite server
    // under `pnpm dev`) don't outlive us and keep holding the port.
    #[cfg(unix)]
    if let Some(pgid) = self.pgid {
      kill_process_group(pgid);
    }

    // Kill the direct child too: the only cleanup on non-Unix, and belt-and-
    // suspenders alongside the group kill on Unix. `start_kill` is non-blocking.
    let _ = self.child.start_kill();
  }
}

/// Best-effort `SIGKILL` of an entire process group; a negative pid targets the
/// group. Errors are ignored — this runs on drop as cleanup.
#[cfg(unix)]
fn kill_process_group(pgid: i32) {
  // SAFETY: `libc::kill` is safe to call here — it only delivers a signal and
  // never dereferences memory; any failure (e.g. the group already exited) is
  // reported via errno, which we intentionally ignore.
  unsafe {
    libc::kill(-pgid, libc::SIGKILL);
  }
}

impl DevServer {
  /// The host the dev server is bound to.
  #[must_use]
  pub fn host(&self) -> &str {
    &self.host
  }

  /// The port the dev server is listening on.
  #[must_use]
  pub fn port(&self) -> u16 {
    self.port
  }

  /// The proxy target URL for this server, e.g. `http://127.0.0.1:5173`.
  #[must_use]
  pub fn target_url(&self) -> String {
    format!("http://{}:{}", self.host, self.port)
  }

  /// Build a [`dev_router`] proxying `prefixes` to this server. Pass `["/"]` to
  /// proxy everything.
  ///
  /// # Panics
  ///
  /// Inherits [`dev_router`]'s panics: a prefix without a leading `/`, or
  /// overlapping non-`"/"` prefixes.
  pub fn router<I, S>(&self, prefixes: I) -> Router
  where
    I: IntoIterator<Item = S>,
    S: Into<String>,
  {
    dev_router(prefixes, &self.target_url())
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn vite_preset_injects_host_and_port() {
    let argv = DevConfig::vite()
      .host("127.0.0.1")
      .port(4321)
      .command_line();

    // Regression guard: Vite binds `localhost` (often IPv6 `::1`) by default,
    // but the readiness probe + proxy target use the configured host. Without
    // this pin nothing can reach the dev server, so it never becomes ready.
    assert!(
      argv.windows(2).any(|w| w == ["--host", "127.0.0.1"]),
      "vite preset must pin --host: {argv:?}"
    );
    assert_eq!(argv.first().map(String::as_str), Some("pnpm"));
    assert!(argv.contains(&"--strictPort".to_string()));
    assert!(argv.contains(&"4321".to_string()));
  }

  #[test]
  fn tool_neutral_config_injects_no_flags() {
    let argv = DevConfig::new(["npm", "run", "dev"])
      .port(3000)
      .command_line();

    assert_eq!(argv, vec!["npm", "run", "dev"]);
  }
}
