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

/// Wait until the dev server accepts connections, giving up as soon as the
/// child exits instead of waiting out the rest of `timeout`.
///
/// A child that has exited is never going to bind the port, so continuing to
/// wait just delays the error and then describes a dev server that died on
/// startup as one that was too slow. Readiness is still a property of the port
/// rather than of the process, though: a command that launches the real dev
/// server and returns leaves it listening, so the port gets one more check
/// before an exit is treated as a failure.
async fn wait_for_startup(
  host: &str,
  port: u16,
  timeout: Duration,
  child: &mut Child,
) -> std::io::Result<()> {
  let exit = tokio::select! {
    result = wait_until_ready(host, port, timeout) => return result,
    exit = child.wait() => exit,
  };

  // A zero timeout probes exactly once: one connect attempt, no retry.
  if wait_until_ready(host, port, Duration::ZERO).await.is_ok() {
    return Ok(());
  }

  Err(match exit {
    Ok(status) => {
      std::io::Error::other(format!("dev server exited before becoming ready: {status}"))
    }
    Err(e) => std::io::Error::other(format!(
      "dev server exited before becoming ready, and waiting on it failed: {e}"
    )),
  })
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
  /// The returned [`DevServer`] owns the child process and shuts it down on
  /// drop. On Unix the child runs in its own process group, so grandchildren
  /// (such as the real Vite server spawned by `pnpm dev`) are reaped too rather
  /// than orphaned; the group gets a `SIGTERM` and a short grace period before
  /// being `SIGKILL`ed (see [`DevServer`]).
  ///
  /// The child's stdin is `/dev/null`. It runs in a background process group, so
  /// it could never usefully read the terminal anyway, and the interactive
  /// machinery in these toolchains is gated on `isatty` (Vite's CLI shortcuts,
  /// pnpm's `setRawMode` keypress handling), so with a null stdin none of it can
  /// take the tty out of canonical mode. Stdout and stderr are still inherited.
  ///
  /// # Errors
  ///
  /// Returns an error if the command is empty
  /// ([`std::io::ErrorKind::InvalidInput`]), if the process fails to spawn (for
  /// example, the program is not found), if the child exits before the port
  /// accepts connections, or if the dev server does not accept connections
  /// before the readiness timeout (see [`DevConfig::ready_timeout`]).
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

    // The child can't read the terminal usefully (on Unix it lives in a
    // background process group, where a tty read just stops it with SIGTTIN), so
    // hand it a null stdin. That also makes `isatty` false, and the interactive
    // machinery that reaches for the terminal is gated on exactly that: Vite
    // binds its CLI shortcuts under `process.stdin.isTTY`, and pnpm's keypress
    // handling calls `setRawMode` under the same check. Whichever runs, it can no
    // longer switch the tty out of canonical mode with echo off and then miss its
    // chance to restore it.
    builder.stdin(std::process::Stdio::null());

    // On Unix, run the dev server in its own process group so drop can reap the
    // whole tree: `pnpm dev` runs the real Vite server as a grandchild, and
    // killing only the direct child would orphan it (leaking the port).
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut builder, 0);

    let mut cmd = Command::from(builder);

    cmd.kill_on_drop(true);

    let mut child = cmd.spawn()?;

    #[cfg(unix)]
    let pgid = child.id().and_then(|id| i32::try_from(id).ok());

    if let Err(e) = wait_for_startup(&self.host, self.port, self.ready_timeout, &mut child).await {
      // Startup failed: reap the whole group before surfacing the error
      // (`kill_on_drop` alone would only get the direct child). This blocks the
      // calling thread for at most `TERM_GRACE`, and only once nothing is left
      // in the group to wait for — a group that has already exited is detected
      // on the first poll and costs nothing.
      #[cfg(unix)]
      if let Some(pgid) = pgid {
        shutdown_process_group(pgid, &mut child);
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

/// Owns the spawned dev server; shuts it down (and, on Unix, its whole process
/// group) when dropped.
///
/// On Unix the group gets a `SIGTERM` and up to a second to exit on its own
/// before being `SIGKILL`ed, so a dev server that installed a signal handler can
/// run its cleanup. `SIGKILL` is uncatchable, so killing outright skips that
/// cleanup entirely, and anything the tool restores on shutdown (terminal modes
/// among them) is left as it was.
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
      shutdown_process_group(pgid, &mut self.child);
    }

    // Kill the direct child too: the only cleanup on non-Unix, and belt-and-
    // suspenders alongside the group shutdown on Unix. `start_kill` is
    // non-blocking, and errors (already exited and reaped) don't matter here.
    let _ = self.child.start_kill();
  }
}

/// How long the process group gets to exit on `SIGTERM` before we escalate to
/// `SIGKILL`. Long enough for a dev server to run its shutdown handler, short
/// enough that a wedged child can't stall the drop.
#[cfg(unix)]
const TERM_GRACE: Duration = Duration::from_secs(1);

/// How often we re-check for exit while waiting out [`TERM_GRACE`].
#[cfg(unix)]
const TERM_POLL_INTERVAL: Duration = Duration::from_millis(20);

/// Best-effort shutdown of an entire process group: `SIGTERM`, then `SIGKILL` if
/// anything is still alive after [`TERM_GRACE`].
///
/// The graceful first step exists so the child can run whatever it does on
/// shutdown; `SIGKILL` is uncatchable, so going straight to it skips every
/// handler the tool installed. Escalating afterwards still preserves the
/// reap-the-whole-group guarantee for children that ignore `SIGTERM`.
///
/// Blocks the calling thread for at most [`TERM_GRACE`]. Errors are ignored,
/// since this runs as cleanup.
#[cfg(unix)]
fn shutdown_process_group(pgid: i32, child: &mut Child) {
  signal_process_group(pgid, libc::SIGTERM);

  let deadline = std::time::Instant::now() + TERM_GRACE;

  loop {
    // Reap the direct child if it has exited: a zombie still counts as a live
    // process for the group check below, so without this we'd always wait out
    // the full grace period and then `SIGKILL` a group that's already gone.
    let _ = child.try_wait();

    if !process_group_exists(pgid) {
      return;
    }

    if std::time::Instant::now() >= deadline {
      break;
    }

    std::thread::sleep(TERM_POLL_INTERVAL);
  }

  signal_process_group(pgid, libc::SIGKILL);
}

/// Send `signal` to an entire process group; a negative pid targets the group.
#[cfg(unix)]
fn signal_process_group(pgid: i32, signal: i32) {
  // SAFETY: `libc::kill` is safe to call here — it only delivers a signal and
  // never dereferences memory; any failure (e.g. the group already exited) is
  // reported via errno, which we intentionally ignore.
  unsafe {
    libc::kill(-pgid, signal);
  }
}

/// Whether any process remains in the group. Signal 0 runs `kill`'s existence
/// and permission checks without delivering anything, so only `ESRCH` ("no such
/// process") means the group is really gone.
#[cfg(unix)]
fn process_group_exists(pgid: i32) -> bool {
  // SAFETY: as in `signal_process_group`, and signal 0 delivers nothing at all.
  let result = unsafe { libc::kill(-pgid, 0) };

  if result == 0 {
    return true;
  }

  std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
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
