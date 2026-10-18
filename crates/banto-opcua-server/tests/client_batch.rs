//! クライアントの部品（ADR-0005）のまとめて Read を、本リポジトリのサーバー相手に確かめる。

use std::{
    num::NonZeroUsize,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use banto_opcua_client::{
    ClientConfig, ConnectedClient, Error as ClientError, OpcUaClient, ReconnectPolicy,
    model::{NodeAddress, ScalarValue, StatusCode},
};
use banto_opcua_server::{
    OpcUaServer, RunningServer, ServerConfig, TagReading,
    model::{DataType, Quality, SampleTimestamps},
};

const TAGS_URI: &str = "urn:banto:opcua:tags";

fn address(text: &str) -> NodeAddress {
    text.parse().expect("NodeAddress を解析できる")
}

async fn start(config: ServerConfig) -> RunningServer {
    OpcUaServer::start(config).await.expect("server starts")
}

async fn connect(config: ClientConfig) -> ConnectedClient {
    OpcUaClient::connect_with(config)
        .await
        .expect("client connects")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn batch_read_returns_each_node_in_request_order() {
    let server = start(ServerConfig::default().with_port(0)).await;
    let sampled_at = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    server
        .add_tag_reading(
            "PLC2",
            "Cell1",
            "Count",
            TagReading::new(
                DataType::Double,
                Some(3.0),
                Quality::Good,
                SampleTimestamps::new(Some(sampled_at), SystemTime::now()),
            ),
        )
        .expect("add tag");
    let client = connect(ClientConfig::new(server.endpoint())).await;
    let namespace = server.namespace_index();

    let readings = client
        .read_batch(&[
            address(&format!("nsu={TAGS_URI};s=PLC2.Cell1.Count")),
            address(&format!("ns={namespace};s=PLC1.Line1.Running")),
            address("i=2259"),
            address(&format!("ns={namespace};s=PLC1.Line1.Missing")),
            address("nsu=urn:missing;i=1"),
            address("i=2255"),
        ])
        .await
        .expect("read batch");

    assert_eq!(readings.len(), 6);
    assert_eq!(readings[0].value, Some(ScalarValue::Double(3.0)));
    assert_eq!(readings[0].status, StatusCode::GOOD);
    assert_eq!(readings[0].source_timestamp, Some(sampled_at));
    assert_eq!(readings[1].value, Some(ScalarValue::Boolean(false)));
    assert_eq!(readings[1].status, StatusCode::GOOD);
    assert_eq!(
        readings[2].value,
        Some(ScalarValue::Int32(0)),
        "ServerState::Running"
    );
    for (position, reading) in readings.iter().enumerate().skip(3).take(2) {
        assert_eq!(reading.value, None, "{position}");
        assert_eq!(
            reading.status,
            StatusCode::BAD_NODE_ID_UNKNOWN,
            "{position}"
        );
    }
    assert_eq!(readings[5].value, None, "NamespaceArray は配列");
    assert_eq!(readings[5].status, StatusCode::BAD_NOT_SUPPORTED);

    assert_eq!(client.read_batch(&[]).await.expect("empty read"), vec![]);

    client.disconnect().await.expect("client disconnects");
    server.shutdown().await.expect("server shuts down");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_is_split_by_the_advertised_max_nodes_per_read() {
    let server = start(
        ServerConfig::default()
            .with_port(0)
            .with_max_nodes_per_read(NonZeroUsize::new(2).unwrap()),
    )
    .await;
    // 上限を超える 1 回の Read は、サーバーが要求ごと拒否する。部品は広告値で分けて
    // 送る（広告値が実際より大きくても、拒否されたら半分にして送り直す）。
    let client = connect(ClientConfig::new(server.endpoint())).await;
    let nodes: Vec<_> = ["i=2259", "i=2255", "i=2256", "i=11705", "i=11707"]
        .map(address)
        .into();

    let readings = client.read_batch(&nodes).await.expect("read batch");

    assert_eq!(readings.len(), nodes.len());
    assert_eq!(readings[0].value, Some(ScalarValue::Int32(0)));
    assert_eq!(readings[1].status, StatusCode::BAD_NOT_SUPPORTED, "配列");
    assert_eq!(readings[2].status, StatusCode::BAD_NOT_SUPPORTED, "構造体");
    assert_eq!(
        readings[3].value,
        Some(ScalarValue::UInt32(2)),
        "MaxNodesPerRead"
    );
    assert_eq!(
        readings[4].value,
        Some(ScalarValue::UInt32(10)),
        "MaxNodesPerWrite"
    );

    client.disconnect().await.expect("client disconnects");
    server.shutdown().await.expect("server shuts down");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn connects_and_reads_when_the_server_allows_one_node_per_read() {
    let server = start(
        ServerConfig::default()
            .with_port(0)
            .with_max_nodes_per_read(NonZeroUsize::new(1).unwrap()),
    )
    .await;
    // 接続時の情報の Read（2 ノード）も、上限 1 のサーバーへ分けて送る。
    let client = connect(ClientConfig::new(server.endpoint())).await;

    let readings = client
        .read_batch(&[
            address("i=2259"),
            address(&format!("nsu={TAGS_URI};s=PLC1.Line1.Running")),
            address("i=11705"),
        ])
        .await
        .expect("read batch");

    assert_eq!(readings[0].value, Some(ScalarValue::Int32(0)));
    assert_eq!(
        readings[1].value,
        Some(ScalarValue::Boolean(false)),
        "nsu= を解決できる"
    );
    assert_eq!(readings[2].value, Some(ScalarValue::UInt32(1)));

    client.disconnect().await.expect("client disconnects");
    server.shutdown().await.expect("server shuts down");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stopped_server_fails_later_calls_without_reconnecting() {
    let server = start(ServerConfig::default().with_port(0)).await;
    let port = server.local_addr().port();
    let client = connect(ClientConfig::new(server.endpoint())).await;
    let nodes = [address("i=2259")];
    client.read_batch(&nodes).await.expect("read before stop");

    server.shutdown().await.expect("server shuts down");
    assert!(client.read_batch(&nodes).await.is_err());

    // 同じポートでサーバーが戻っても、張り直さない。
    let restarted = start(ServerConfig::default().with_port(port)).await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert!(matches!(
        client.read_batch(&nodes).await,
        Err(ClientError::Disconnected)
    ));

    client.disconnect().await.expect("disconnect after loss");
    restarted.shutdown().await.expect("server shuts down");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn automatic_reconnect_rereads_the_namespace_array() {
    let server = start(ServerConfig::default().with_port(0)).await;
    let port = server.local_addr().port();
    let client = connect(ClientConfig::new(server.endpoint()).with_reconnect(
        ReconnectPolicy::Automatic {
            initial_delay: Duration::from_millis(200),
            max_delay: Duration::from_secs(1),
            max_retries: None,
        },
    ))
    .await;
    let renamed = [address("nsu=urn:banto:opcua:renamed;s=PLC1.Line1.Speed")];
    let before = client
        .read_batch(&renamed)
        .await
        .expect("read before restart");
    assert_eq!(before[0].status, StatusCode::BAD_NODE_ID_UNKNOWN);

    server.shutdown().await.expect("server shuts down");
    let restarted = start(
        ServerConfig::default()
            .with_port(port)
            .with_namespace_uri("urn:banto:opcua:renamed"),
    )
    .await;

    let after = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            match client.read_batch(&renamed).await {
                Ok(readings) => break readings,
                Err(ClientError::Disconnected | ClientError::Read(_)) => {
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
                Err(other) => panic!("想定外の失敗: {other}"),
            }
        }
    })
    .await
    .expect("自動再接続で張り直す");
    assert_eq!(after[0].status, StatusCode::GOOD);
    assert_eq!(after[0].value, Some(ScalarValue::Double(0.0)));

    client.disconnect().await.expect("client disconnects");
    restarted.shutdown().await.expect("server shuts down");
}
