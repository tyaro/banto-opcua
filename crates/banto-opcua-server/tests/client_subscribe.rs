//! クライアントの購読モード（ADR-0006）を、本リポジトリのサーバー相手に確かめる。

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use banto_opcua_client::{
    ClientConfig, ConnectedClient, Error as ClientError, NodeReading, OpcUaClient, ReconnectPolicy,
    SubscriptionSettings,
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

fn fast_subscription() -> SubscriptionSettings {
    SubscriptionSettings::default()
        .with_publishing_interval(Duration::from_millis(100))
        .with_sampling_interval(Duration::from_millis(50))
}

async fn start(config: ServerConfig) -> RunningServer {
    OpcUaServer::start(config).await.expect("server starts")
}

async fn connect(config: ClientConfig) -> ConnectedClient {
    OpcUaClient::connect_with(config.with_subscription(fast_subscription()))
        .await
        .expect("client connects")
}

fn reading(value: f64, sampled_at: SystemTime) -> TagReading {
    TagReading::new(
        DataType::Double,
        Some(value),
        Quality::Good,
        SampleTimestamps::new(Some(sampled_at), SystemTime::now()),
    )
}

/// `latest` が条件を満たすまで待つ。
async fn wait_for(
    client: &ConnectedClient,
    mut done: impl FnMut(&[NodeReading]) -> bool,
) -> Vec<NodeReading> {
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if let Ok(readings) = client.latest().await
                && done(&readings)
            {
                break readings;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("条件を満たす最新値が届く")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn latest_follows_notifications_in_registration_order() {
    let server = start(ServerConfig::default().with_port(0)).await;
    let first = UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    server
        .add_tag_reading("PLC2", "Cell1", "Count", reading(3.0, first))
        .expect("add tag");
    let client = connect(ClientConfig::new(server.endpoint())).await;
    assert_eq!(client.latest().await.expect("未登録"), vec![]);
    let namespace = server.namespace_index();

    let statuses = client
        .monitor(&[
            address(&format!("nsu={TAGS_URI};s=PLC2.Cell1.Count")),
            address(&format!("ns={namespace};s=PLC1.Line1.Running")),
            // async-opcua のサーバーは、無いノードの MonitoredItem も作り、最初の通知で
            // BadNodeIdUnknown を送る。本リポジトリのタグも同じ（削除と復帰の購読）。
            address("i=999999"),
            address("nsu=urn:missing;i=1"),
            address(&format!("ns={namespace};s=PLC1.Line1.Missing")),
        ])
        .await
        .expect("monitor");
    assert_eq!(
        statuses,
        vec![
            StatusCode::GOOD,
            StatusCode::GOOD,
            StatusCode::GOOD,
            StatusCode::BAD_NODE_ID_UNKNOWN,
            StatusCode::GOOD,
        ],
        "解決できない nsu= だけは作らずに BadNodeIdUnknown"
    );

    let readings = wait_for(&client, |readings| {
        readings[0].value == Some(ScalarValue::Double(3.0))
            && readings[1].value == Some(ScalarValue::Boolean(false))
            && readings[2].status == StatusCode::BAD_NODE_ID_UNKNOWN
            && readings[4].status == StatusCode::BAD_NODE_ID_UNKNOWN
    })
    .await;
    assert_eq!(readings[0].status, StatusCode::GOOD);
    assert_eq!(readings[0].source_timestamp, Some(first));
    for reading in &readings[2..] {
        assert_eq!(reading.value, None);
        assert_eq!(reading.status, StatusCode::BAD_NODE_ID_UNKNOWN);
    }

    let second = first + Duration::from_secs(1);
    server
        .update_tag_reading("PLC2", "Cell1", "Count", reading(4.0, second))
        .expect("update tag");
    let readings = wait_for(&client, |readings| {
        readings[0].value == Some(ScalarValue::Double(4.0))
    })
    .await;
    assert_eq!(readings[0].source_timestamp, Some(second));
    assert_eq!(readings[3].status, StatusCode::BAD_NODE_ID_UNKNOWN);

    client.disconnect().await.expect("client disconnects");
    server.shutdown().await.expect("server shuts down");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn items_beyond_the_subscription_limit_are_reported_per_node() {
    let server = start(ServerConfig::default().with_port(0)).await;
    let client = connect(ClientConfig::new(server.endpoint())).await;
    // async-opcua のサーバーは 1 つの購読に 1000 ノード、1 回の要求に 1000 ノードまで。
    let nodes = vec![address("i=2259"); 1001];

    let statuses = client.monitor(&nodes).await.expect("monitor");

    assert_eq!(statuses.len(), 1001);
    assert!(
        statuses[..1000]
            .iter()
            .all(|status| *status == StatusCode::GOOD)
    );
    assert_eq!(statuses[1000], StatusCode::BAD_TOO_MANY_MONITORED_ITEMS);
    let readings = wait_for(&client, |readings| {
        readings[999].value == Some(ScalarValue::Int32(0))
    })
    .await;
    assert_eq!(
        readings[1000].status,
        StatusCode::BAD_TOO_MANY_MONITORED_ITEMS
    );
    assert_eq!(readings[1000].value, None);

    client.disconnect().await.expect("client disconnects");
    server.shutdown().await.expect("server shuts down");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn latest_fails_after_the_connection_is_lost() {
    let server = start(ServerConfig::default().with_port(0)).await;
    let client = connect(ClientConfig::new(server.endpoint())).await;
    client.monitor(&[address("i=2259")]).await.expect("monitor");
    wait_for(&client, |readings| readings[0].status == StatusCode::GOOD).await;

    server.shutdown().await.expect("server shuts down");
    tokio::time::timeout(Duration::from_secs(30), async {
        while client.latest().await.is_ok() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .expect("切断の後は接続ごとの失敗になる");
    assert!(matches!(
        client.latest().await,
        Err(ClientError::Disconnected)
    ));

    client.disconnect().await.expect("disconnect after loss");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn automatic_reconnect_recreates_the_subscription_with_the_new_namespace_array() {
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
    let statuses = client
        .monitor(&[address("nsu=urn:banto:opcua:renamed;s=PLC1.Line1.Speed")])
        .await
        .expect("monitor");
    assert_eq!(
        statuses,
        vec![StatusCode::BAD_NODE_ID_UNKNOWN],
        "最初のサーバーには renamed の名前空間が無い"
    );

    server.shutdown().await.expect("server shuts down");
    let restarted = start(
        ServerConfig::default()
            .with_port(port)
            .with_namespace_uri("urn:banto:opcua:renamed"),
    )
    .await;

    let readings = wait_for(&client, |readings| readings[0].status == StatusCode::GOOD).await;
    assert_eq!(readings[0].value, Some(ScalarValue::Double(0.0)));

    // 再起動したサーバーは購読の ID を 1 から振り直すので、作り直した購読は古い購読と
    // 同じ ID になる。作り直した購読が生きていて、その後の変化も届くことを確かめる。
    restarted.set_fixed_speed(42.5).expect("update speed");
    wait_for(&client, |readings| {
        readings[0].value == Some(ScalarValue::Double(42.5))
    })
    .await;

    client.disconnect().await.expect("client disconnects");
    restarted.shutdown().await.expect("server shuts down");
}
