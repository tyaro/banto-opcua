//! Discovery だけを中継し、セッション用の TCP 応答を止めて接続失敗を再現する。

use std::{net::Ipv4Addr, time::Duration};

use banto_opcua_client::{Error as ClientError, OpcUaClient};
use banto_opcua_server::{OpcUaServer, ServerConfig};
use tokio::{
    io::{AsyncReadExt, copy_bidirectional},
    net::{TcpListener, TcpStream},
};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stalled_session_closes_after_timeout_and_cancellation() {
    // 同じポートで Discovery とセッションを別のループバック IP へ振り分ける。
    let config = ServerConfig::default()
        .with_bind(Ipv4Addr::new(127, 0, 0, 2).into())
        .with_port(0);
    let server = OpcUaServer::start(config).await.expect("server starts");
    let backend = server.local_addr();
    let proxy = TcpListener::bind((Ipv4Addr::LOCALHOST, backend.port()))
        .await
        .expect("proxy binds to the same port on another loopback IP");
    let endpoint = format!("opc.tcp://127.0.0.1:{}/", backend.port());

    for cancel in [false, true] {
        let endpoint = endpoint.clone();
        let session_timeout = if cancel {
            Duration::from_secs(10)
        } else {
            Duration::from_secs(2)
        };
        let connection = tokio::spawn(async move {
            OpcUaClient::connect_with_session_timeout(&endpoint, session_timeout).await
        });

        // CI では接続前のサンプル鍵生成に時間がかかることがある。
        let (mut discovery, _) = tokio::time::timeout(Duration::from_secs(45), proxy.accept())
            .await
            .expect("Discovery connection arrives")
            .expect("accept Discovery connection");
        let mut backend_discovery = TcpStream::connect(backend)
            .await
            .expect("connect to server for Discovery");
        let relay = tokio::spawn(async move {
            copy_bidirectional(&mut discovery, &mut backend_discovery).await
        });

        let (mut stalled_session, _) =
            tokio::time::timeout(Duration::from_secs(10), proxy.accept())
                .await
                .expect("session connection arrives")
                .expect("accept session connection");
        let mut bytes = [0u8; 4096];
        let hello_len =
            tokio::time::timeout(Duration::from_secs(5), stalled_session.read(&mut bytes))
                .await
                .expect("session sends a HEL message")
                .expect("read session HEL message");
        assert!(hello_len > 0);

        if cancel {
            connection.abort();
            assert!(
                connection
                    .await
                    .expect_err("connection future was cancelled")
                    .is_cancelled()
            );
        } else {
            let result = tokio::time::timeout(Duration::from_secs(5), connection)
                .await
                .expect("connection timeout returns promptly")
                .expect("connection task completes");
            assert!(
                matches!(result, Err(ClientError::Connect(message)) if message.contains("時間切れ")),
                "session should time out"
            );
        }

        let closed = tokio::time::timeout(Duration::from_secs(2), stalled_session.read(&mut bytes))
            .await
            .expect("session socket closes promptly")
            .expect("read session socket after failure");
        assert_eq!(closed, 0, "event loop left the session socket open");
        relay.abort();
        let _ = relay.await;
    }

    server.shutdown().await.expect("server shuts down");
}
