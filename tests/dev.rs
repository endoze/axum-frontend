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
  // Bind then drop to obtain a very likely-closed port number. "Very likely" is
  // the catch: the OS can hand that number straight back to another test binding
  // an ephemeral port, and then the probe legitimately succeeds. Retry on a fresh
  // port instead of failing the run over it.
  for _ in 0..5 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    drop(listener);

    let result =
      axum_frontend::wait_until_ready("127.0.0.1", port, Duration::from_millis(300)).await;

    if result.is_err() {
      return;
    }
  }

  panic!("closed port should time out, but every candidate port was re-bound");
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

/// Write an executable-by-`sh` fixture script under a fresh temp directory and
/// return `(dir, script_path)`. The caller removes `dir` when it's done.
#[cfg(unix)]
fn write_fixture(name: &str, body: &str) -> (std::path::PathBuf, std::path::PathBuf) {
  let dir = std::env::temp_dir().join(format!("axum-frontend-{}-{name}", std::process::id()));

  std::fs::create_dir_all(&dir).unwrap();

  let script = dir.join("run.sh");

  std::fs::write(&script, body).unwrap();

  (dir, script)
}

/// Spawn a fixture script as a `DevServer`. Readiness is satisfied by a listener
/// the test itself holds, so the script doesn't have to bind anything. This is
/// about process lifecycle, not about proxying.
#[cfg(unix)]
async fn spawn_fixture(
  script: &std::path::Path,
) -> (axum_frontend::DevServer, tokio::net::TcpListener) {
  let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
  let port = listener.local_addr().unwrap().port();
  let server = axum_frontend::DevConfig::new(["sh", script.to_str().unwrap()])
    .port(port)
    .ready_timeout(Duration::from_secs(5))
    .spawn()
    .await
    .expect("fixture should spawn and the test listener should satisfy readiness");

  (server, listener)
}

/// Poll until `path` appears, up to `timeout`.
///
/// The fixtures use this to announce that they've finished setting up. Without
/// it the tests race the shell's own startup: `spawn` returns the moment the
/// port is reachable, which here is immediately, so a signal can land before the
/// script has run its first line.
#[cfg(unix)]
async fn wait_for_file(path: &std::path::Path, timeout: Duration) {
  let deadline = std::time::Instant::now() + timeout;

  while !path.exists() {
    assert!(
      std::time::Instant::now() < deadline,
      "fixture never created {}",
      path.display()
    );

    tokio::time::sleep(Duration::from_millis(20)).await;
  }
}

/// Whether `pid` is a process that is still running.
///
/// A killed process sticks around as a zombie until its parent reaps it, and a
/// grandchild killed with its group is guaranteed to be orphaned first: its
/// parent dies in the same signal, so it gets reparented to PID 1. Whether that
/// zombie then disappears is up to PID 1, and under CI's container it never
/// does, because GitHub runs the job container as `tail -f /dev/null`, which
/// never `wait`s. `kill -0` succeeds for a zombie, so on Linux we read the
/// process state instead and treat `Z` as gone; a zombie has already died,
/// which is all this test is asking about.
#[cfg(target_os = "linux")]
fn is_running(pid: &str) -> bool {
  let Ok(stat) = std::fs::read_to_string(format!("/proc/{pid}/stat")) else {
    return false;
  };

  // Field 2 is the executable name in parentheses and may itself contain
  // spaces, so the state is the first field after the *last* `)`.
  let Some((_, after_comm)) = stat.rsplit_once(") ") else {
    return false;
  };

  !after_comm.starts_with('Z')
}

/// Whether `pid` is a process that is still running. `kill -0` succeeds only
/// while the process exists and we may signal it.
///
/// Zombies (see the Linux version above) aren't a concern here: every init we
/// would be reparented to on these platforms reaps promptly.
#[cfg(all(unix, not(target_os = "linux")))]
fn is_running(pid: &str) -> bool {
  std::process::Command::new("kill")
    .args(["-0", pid])
    .stderr(std::process::Stdio::null())
    .status()
    .map(|s| s.success())
    .unwrap_or(false)
}

/// Poll until `pid` is no longer running, up to `timeout`.
#[cfg(unix)]
fn wait_for_pid_to_exit(pid: &str, timeout: Duration) -> bool {
  let deadline = std::time::Instant::now() + timeout;

  loop {
    if !is_running(pid) {
      return true;
    }

    if std::time::Instant::now() >= deadline {
      return false;
    }

    std::thread::sleep(Duration::from_millis(20));
  }
}

/// Dropping a `DevServer` must give the child a chance to run its own shutdown
/// before anything uncatchable arrives. `SIGKILL` skips every handler the child
/// installed, so whatever it would have torn down (terminal modes, sockets,
/// lockfiles, temp dirs) is left behind.
#[cfg(unix)]
#[tokio::test]
async fn drop_lets_the_child_handle_sigterm_before_killing() {
  // A background subshell publishes "ready" a beat after launch, so by the time
  // the test signals, the script is provably parked in `wait`. Signalling `sh`
  // while it's still waiting on a *foreground* child gets the signal propagated
  // rather than trapped. That quirk is shells-only (Node services SIGTERM from
  // its event loop), but it makes this fixture flaky if the script announces
  // itself ready from the main line of execution.
  let (dir, script) = write_fixture(
    "sigterm",
    "trap ': > \"$0.cleaned\"; exit 0' TERM\n(sleep 0.2; : > \"$0.ready\") &\nsleep 300 &\nwait\n",
  );
  let marker = script.with_extension("sh.cleaned");
  let (server, _listener) = spawn_fixture(&script).await;

  wait_for_file(&script.with_extension("sh.ready"), Duration::from_secs(5)).await;

  drop(server);

  assert!(
    marker.exists(),
    "child should have run its TERM handler before being killed"
  );

  std::fs::remove_dir_all(&dir).ok();
}

/// The escalation must not weaken the guarantee the group kill exists for:
/// `pnpm dev` runs the real dev server as a grandchild, so killing only the
/// direct child orphans it and leaks the port. A child that ignores `SIGTERM`
/// still has to die, along with everything else in its group.
#[cfg(unix)]
#[tokio::test]
async fn drop_kills_the_whole_group_even_when_sigterm_is_ignored() {
  let (dir, script) = write_fixture(
    "stubborn",
    "trap '' TERM\nsleep 300 &\necho $! > \"$0.grandchild\"\nwait\n",
  );
  let pid_file = script.with_extension("sh.grandchild");
  let (server, _listener) = spawn_fixture(&script).await;

  wait_for_file(&pid_file, Duration::from_secs(5)).await;

  let grandchild = std::fs::read_to_string(&pid_file)
    .expect("fixture should have recorded its grandchild pid")
    .trim()
    .to_string();

  drop(server);

  assert!(
    wait_for_pid_to_exit(&grandchild, Duration::from_secs(5)),
    "grandchild {grandchild} should have been killed with the rest of the group"
  );

  std::fs::remove_dir_all(&dir).ok();
}

/// A dev server that dies during startup has to be reported when it dies. It
/// can never become ready once it's gone, so waiting out the readiness timeout
/// only delays the error, and reports a crash as "not ready" while it's at it.
#[cfg(unix)]
#[tokio::test]
async fn spawn_fails_fast_when_the_child_exits_before_becoming_ready() {
  let (dir, script) = write_fixture("early-exit", "exit 3\n");
  // Nothing listens on port 1: it's outside the ephemeral range, so no
  // concurrent test can be handed it, and it hosts no service anyone runs.
  let started = std::time::Instant::now();
  let result = axum_frontend::DevConfig::new(["sh", script.to_str().unwrap()])
    .port(1)
    .ready_timeout(Duration::from_secs(10))
    .spawn()
    .await;
  let error = result.expect_err("a child that has exited can never become ready");

  assert!(
    started.elapsed() < Duration::from_secs(5),
    "should report the exit as it happens, not wait out the readiness timeout"
  );
  assert!(
    error.to_string().contains("exited"),
    "error should say the dev server exited: {error}"
  );

  std::fs::remove_dir_all(&dir).ok();
}

/// Readiness is a property of the port, not of the child: a command that
/// launches the real dev server and returns leaves something listening, and
/// that's a running dev server, not a crash.
#[cfg(unix)]
#[tokio::test]
async fn spawn_succeeds_when_the_child_exits_but_the_port_is_open() {
  let (dir, script) = write_fixture("exit-after-ready", "exit 0\n");
  // Held open for the whole test, standing in for the server the command would
  // have left behind.
  let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
  let port = listener.local_addr().unwrap().port();
  let server = axum_frontend::DevConfig::new(["sh", script.to_str().unwrap()])
    .port(port)
    .ready_timeout(Duration::from_secs(5))
    .spawn()
    .await;

  assert!(
    server.is_ok(),
    "port is open, so the dev server is ready no matter what the child did: {:?}",
    server.err()
  );

  std::fs::remove_dir_all(&dir).ok();
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
