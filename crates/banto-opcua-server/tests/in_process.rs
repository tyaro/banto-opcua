//! 同一プロセスでサーバーを起動し、クライアントがループバックへ接続する。
//!
//! PR の CI（`cargo test`）が毎回実行する層。外部の Python クライアントや
//! opc-plc コンテナは `.github/workflows/interop.yml` の別ワークフロー。

use std::net::Ipv4Addr;

use banto_opcua_client::OpcUaClient;
use banto_opcua_server::{OpcUaServer, ServerConfig};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn client_reads_server_state_on_loopback() {
    let server = OpcUaServer::start(ServerConfig {
        port: 0,
        ..ServerConfig::default()
    })
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
