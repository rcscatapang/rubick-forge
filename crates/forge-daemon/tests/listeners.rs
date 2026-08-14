//! What the daemon actually listens on.
//!
//! The tailnet bind is the one feature whose whole point is which sockets
//! exist, so these tests bind real ones. The IPv6 loopback stands in for a
//! Tailscale address: it is a second address every Mac has, so two listeners can be
//! proved without a tailnet. (macOS will not bind `127.0.0.2` without an lo0
//! alias, so that more obvious stand-in is not available.)

use std::net::{SocketAddr, TcpListener as StdListener};

use forge_daemon::paths::StateDir;
use forge_daemon::server::Daemon;

/// A port nothing is using, released again before the daemon takes it.
fn free_port() -> u16 {
    StdListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Bootstrap a daemon in a throwaway state directory with the given config.
fn daemon_with(config: &str) -> (Daemon, tempfile::TempDir) {
    let temp = tempfile::tempdir().unwrap();
    let state_dir = StateDir::at(temp.path());
    state_dir.ensure().unwrap();
    std::fs::write(state_dir.config_path(), config).unwrap();

    (Daemon::bootstrap(&state_dir).unwrap(), temp)
}

/// `GET /health` over a real socket. `Ok(())` means something answered.
async fn health(addr: SocketAddr) -> Result<String, std::io::Error> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let mut stream = tokio::net::TcpStream::connect(addr).await?;
    let request = format!("GET /health HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    stream.write_all(request.as_bytes()).await?;

    let mut response = String::new();
    stream.read_to_string(&mut response).await?;
    Ok(response)
}

/// `GET /health` once the daemon has got round to binding.
///
/// `serve_until` binds on its own task, so a client racing it would find
/// nothing there yet — which is not the thing under test.
async fn health_once_up(addr: SocketAddr) -> String {
    for _ in 0..100 {
        if let Ok(response) = health(addr).await {
            return response;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }

    panic!("nothing answered on {addr} within two seconds");
}

#[tokio::test]
async fn without_a_tailnet_bind_only_loopback_answers() {
    let port = free_port();
    let (daemon, _temp) = daemon_with(&format!("port = {port}\n"));

    let loopback: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let other: SocketAddr = format!("[::1]:{port}").parse().unwrap();

    let (stop, stopped) = tokio::sync::oneshot::channel();
    let serving = tokio::spawn(daemon.serve_until(async {
        let _ = stopped.await;
    }));

    assert!(health_once_up(loopback).await.contains("200 OK"));
    // Nothing bound it, so the connection is refused rather than answered.
    assert!(health(other).await.is_err());

    let _ = stop.send(());
    serving.await.unwrap().unwrap();
}

#[tokio::test]
async fn a_second_bind_answers_without_costing_loopback() {
    let port = free_port();
    let (daemon, _temp) = daemon_with(&format!("port = {port}\ntailscale_bind = \"::1\"\n"));

    let loopback: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let second: SocketAddr = format!("[::1]:{port}").parse().unwrap();

    let (stop, stopped) = tokio::sync::oneshot::channel();
    let serving = tokio::spawn(daemon.serve_until(async {
        let _ = stopped.await;
    }));

    // The same daemon, the same database, reached two ways.
    assert!(health_once_up(loopback).await.contains("200 OK"));
    assert!(health_once_up(second).await.contains("200 OK"));

    // And one signal ends both.
    let _ = stop.send(());
    serving.await.unwrap().unwrap();

    assert!(health(loopback).await.is_err());
    assert!(health(second).await.is_err());
}
