//! クライアントの部品（ADR-0005）のまとめて Write を、本リポジトリのサーバー相手に確かめる。

use std::{num::NonZeroUsize, sync::Arc, time::Duration};

use banto_opcua_client::{
    ClientConfig, ConnectedClient, Error as ClientError, OpcUaClient, ReconnectPolicy,
    WriteOutcome,
    model::{NodeAddress, ScalarValue, StatusCode},
};
use banto_opcua_server::{
    OpcUaServer, RunningServer, ServerConfig, WriteFuture, WriteHandler,
    model::{MappedValue, WriteRejection, WriteRequest},
};
use tokio::sync::mpsc;

const TAGS_URI: &str = "urn:banto:opcua:tags";
const CONNECTION: &str = "PLC2";
const GROUP: &str = "Cell1";

/// 受けた書き込みを記録する。`Locked` は書き込みを受け付けていないタグとして拒否する。
struct Recorder(mpsc::UnboundedSender<(String, MappedValue)>);

impl WriteHandler for Recorder {
    fn write(&self, request: WriteRequest) -> WriteFuture {
        self.0
            .send((request.tag.clone(), request.value))
            .expect("書き込みを記録する");
        Box::pin(async move {
            if request.tag == "Locked" {
                Err(WriteRejection::OutOfService)
            } else {
                Ok(())
            }
        })
    }
}

async fn start(
    config: ServerConfig,
    writable: &[(&str, MappedValue)],
) -> (
    RunningServer,
    mpsc::UnboundedReceiver<(String, MappedValue)>,
) {
    let (calls, received) = mpsc::unbounded_channel();
    let server = OpcUaServer::start_with_write_handler(config, Arc::new(Recorder(calls)))
        .await
        .expect("server starts");
    for (tag, value) in writable {
        server
            .add_tag(CONNECTION, GROUP, tag, *value)
            .expect("add tag");
        server
            .set_tag_writable(CONNECTION, GROUP, tag, true)
            .expect("set writable");
    }
    (server, received)
}

async fn connect(server: &RunningServer) -> ConnectedClient {
    OpcUaClient::connect_with(ClientConfig::new(server.endpoint()))
        .await
        .expect("client connects")
}

fn tag(name: &str) -> NodeAddress {
    format!("nsu={TAGS_URI};s={CONNECTION}.{GROUP}.{name}")
        .parse()
        .expect("NodeAddress を解析できる")
}

fn drain(
    received: &mut mpsc::UnboundedReceiver<(String, MappedValue)>,
) -> Vec<(String, MappedValue)> {
    let mut calls = Vec::new();
    while let Ok(call) = received.try_recv() {
        calls.push(call);
    }
    calls
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn write_batch_returns_each_node_in_request_order() {
    let (server, mut received) = start(
        ServerConfig::default().with_port(0),
        &[
            ("Setpoint", MappedValue::Double(0.0)),
            ("Enable", MappedValue::Boolean(false)),
            ("Locked", MappedValue::Double(0.0)),
        ],
    )
    .await;
    server
        .add_tag(CONNECTION, GROUP, "ReadOnly", MappedValue::Double(0.0))
        .expect("add read-only tag");
    let client = connect(&server).await;
    let namespace = server.namespace_index();
    let by_index: NodeAddress = format!("ns={namespace};s={CONNECTION}.{GROUP}.Enable")
        .parse()
        .unwrap();

    let statuses = client
        .write_batch(&[
            (tag("Setpoint"), ScalarValue::Double(12.5)),
            (by_index, ScalarValue::Boolean(true)),
            (tag("ReadOnly"), ScalarValue::Double(1.0)),
            (tag("Setpoint"), ScalarValue::Int32(1)),
            (tag("Locked"), ScalarValue::Double(1.0)),
            (tag("Missing"), ScalarValue::Double(1.0)),
            (
                "nsu=urn:missing;i=1".parse().unwrap(),
                ScalarValue::Double(1.0),
            ),
        ])
        .await
        .expect("write batch");

    assert_eq!(
        statuses,
        vec![
            StatusCode::GOOD,
            StatusCode::GOOD,
            StatusCode::BAD_NOT_WRITABLE,
            StatusCode::BAD_TYPE_MISMATCH,
            StatusCode::BAD_OUT_OF_SERVICE,
            StatusCode::BAD_NODE_ID_UNKNOWN,
            StatusCode::BAD_NODE_ID_UNKNOWN,
        ]
    );
    let mut calls = drain(&mut received);
    calls.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        calls,
        vec![
            ("Enable".to_owned(), MappedValue::Boolean(true)),
            ("Locked".to_owned(), MappedValue::Double(1.0)),
            ("Setpoint".to_owned(), MappedValue::Double(12.5)),
        ],
        "サーバーの検査を通ったノードだけがハンドラーへ届く"
    );

    assert_eq!(client.write_batch(&[]).await.expect("empty write"), vec![]);

    client.disconnect().await.expect("client disconnects");
    server.shutdown().await.expect("server shuts down");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn write_is_split_by_the_advertised_max_nodes_per_write() {
    let names = ["A", "B", "C", "D", "E"];
    let writable: Vec<_> = names
        .iter()
        .map(|name| (*name, MappedValue::Double(0.0)))
        .collect();
    let (server, mut received) = start(
        ServerConfig::default()
            .with_port(0)
            .with_max_nodes_per_write(NonZeroUsize::new(2).unwrap()),
        &writable,
    )
    .await;
    // 上限を超える 1 回の Write は、サーバーが要求ごと BadTooManyOperations で拒否する。
    let client = connect(&server).await;
    let writes: Vec<_> = names
        .iter()
        .enumerate()
        .map(|(index, name)| (tag(name), ScalarValue::Double(index as f64)))
        .collect();

    let statuses = client.write_batch(&writes).await.expect("write batch");

    assert_eq!(statuses, vec![StatusCode::GOOD; names.len()]);
    assert_eq!(drain(&mut received).len(), names.len());

    client.disconnect().await.expect("client disconnects");
    server.shutdown().await.expect("server shuts down");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn write_after_the_connection_is_lost_reports_every_node_as_not_sent() {
    let writable = [("Setpoint", MappedValue::Double(0.0))];
    let (server, _received) = start(ServerConfig::default().with_port(0), &writable).await;
    let port = server.local_addr().port();
    let client = connect(&server).await;
    let writes = [
        (tag("Setpoint"), ScalarValue::Double(1.0)),
        (tag("Setpoint"), ScalarValue::Double(2.0)),
    ];
    client
        .write_batch(&writes)
        .await
        .expect("write before stop");

    server.shutdown().await.expect("server shuts down");
    let failure = client
        .write_batch(&writes)
        .await
        .expect_err("切断の後の Write は失敗する");
    assert_eq!(failure.outcomes.len(), writes.len());

    // 同じポートでサーバーが戻っても張り直さず、どのノードも送らない。
    let (restarted, mut received) = start(ServerConfig::default().with_port(port), &writable).await;
    tokio::time::sleep(Duration::from_secs(3)).await;
    let failure = client.write_batch(&writes).await.expect_err("張り直さない");
    assert!(matches!(failure.error, ClientError::Disconnected));
    assert_eq!(failure.outcomes, vec![WriteOutcome::NotSent; writes.len()]);
    assert!(drain(&mut received).is_empty());

    client.disconnect().await.expect("disconnect after loss");
    restarted.shutdown().await.expect("server shuts down");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn automatic_reconnect_writes_with_the_new_session_and_namespace_array() {
    let writable = [("Setpoint", MappedValue::Double(0.0))];
    let (server, _received) = start(ServerConfig::default().with_port(0), &writable).await;
    let port = server.local_addr().port();
    let client = OpcUaClient::connect_with(ClientConfig::new(server.endpoint()).with_reconnect(
        ReconnectPolicy::Automatic {
            initial_delay: Duration::from_millis(200),
            max_delay: Duration::from_secs(1),
            max_retries: None,
        },
    ))
    .await
    .expect("client connects");
    let renamed: NodeAddress =
        format!("nsu=urn:banto:opcua:renamed;s={CONNECTION}.{GROUP}.Setpoint")
            .parse()
            .unwrap();
    let writes = [(renamed, ScalarValue::Double(7.0))];
    assert_eq!(
        client
            .write_batch(&writes)
            .await
            .expect("write before restart"),
        vec![StatusCode::BAD_NODE_ID_UNKNOWN],
        "最初のサーバーには renamed の名前空間が無い"
    );

    // 名前空間 URI を変えて同じポートで立て直す。張り直した後は、新しいセッションの
    // トークンと NamespaceArray で書く。
    server.shutdown().await.expect("server shuts down");
    let (restarted, mut received) = start(
        ServerConfig::default()
            .with_port(port)
            .with_namespace_uri("urn:banto:opcua:renamed"),
        &writable,
    )
    .await;
    let statuses = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            match client.write_batch(&writes).await {
                Ok(statuses) => break statuses,
                Err(failure) => {
                    assert!(
                        failure
                            .outcomes
                            .iter()
                            .all(|outcome| outcome.is_definitely_not_written()
                                || matches!(outcome, WriteOutcome::Unknown(_))),
                        "{failure:?}"
                    );
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
            }
        }
    })
    .await
    .expect("自動再接続で張り直す");
    assert_eq!(statuses, vec![StatusCode::GOOD]);
    assert!(drain(&mut received).contains(&("Setpoint".to_owned(), MappedValue::Double(7.0))));

    client.disconnect().await.expect("client disconnects");
    restarted.shutdown().await.expect("server shuts down");
}
