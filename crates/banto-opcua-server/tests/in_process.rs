//! 同一プロセスでサーバーを起動し、クライアントがループバックへ接続する。
//!
//! PR の CI（`cargo test`）が毎回実行する層。外部の Python クライアントや
//! opc-plc コンテナは `.github/workflows/interop.yml` の別ワークフロー。

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use banto_opcua_client::OpcUaClient;
use banto_opcua_server::{OpcUaServer, ServerConfig};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn client_reads_server_state_on_loopback() {
    let server = OpcUaServer::start(ServerConfig::default().with_port(0))
        .await
        .expect("server starts");

    let addr = server.local_addr();
    assert_eq!(addr.ip(), Ipv4Addr::LOCALHOST);
    assert_ne!(addr.port(), 0);
    assert!(server.endpoint().starts_with("opc.tcp://127.0.0.1:"));

    let client = OpcUaClient::connect(server.endpoint())
        .await
        .expect("client connects");
    let state = client
        .read_server_state()
        .await
        .expect("read ServerStatus/State");
    assert_eq!(state, 0, "ServerState::Running");

    client.disconnect().await.expect("client disconnects");
    server.shutdown().await.expect("server shuts down");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn client_reads_server_state_on_ipv6_loopback() {
    if !ipv6_loopback_available() {
        return;
    }

    let server = OpcUaServer::start(
        ServerConfig::default()
            .with_bind(IpAddr::V6(Ipv6Addr::LOCALHOST))
            .with_port(0),
    )
    .await
    .expect("server starts");

    assert!(
        server.endpoint().starts_with("opc.tcp://[::1]:"),
        "{}",
        server.endpoint()
    );

    let client = OpcUaClient::connect(server.endpoint())
        .await
        .expect("client connects");
    let state = client
        .read_server_state()
        .await
        .expect("read ServerStatus/State");
    assert_eq!(state, 0, "ServerState::Running");

    client.disconnect().await.expect("client disconnects");
    server.shutdown().await.expect("server shuts down");
}

/// `[::1]` へ bind できなければ IPv6 テストをスキップする。
///
/// `BANTO_OPCUA_REQUIRE_IPV6=1` のときはスキップせず失敗する。ubuntu の CI
/// だけがこの変数を置く。
fn ipv6_loopback_available() -> bool {
    match std::net::TcpListener::bind((Ipv6Addr::LOCALHOST, 0)) {
        Ok(listener) => {
            drop(listener);
            true
        }
        Err(err) => {
            let required = std::env::var("BANTO_OPCUA_REQUIRE_IPV6").ok().as_deref() == Some("1");
            let message = if required {
                format!(
                    "IPv6 の [::1] へ bind できません。BANTO_OPCUA_REQUIRE_IPV6=1 のためスキップせず失敗します: {err}"
                )
            } else {
                format!("IPv6 の [::1] へ bind できないためスキップします: {err}")
            };
            eprintln!("{message}");
            assert!(!required, "{message}");
            false
        }
    }
}
