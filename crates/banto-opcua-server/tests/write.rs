//! 書き込みハンドラーを通す Write（ADR-0004）をプロセス内で確かめる。

use std::{
    num::NonZeroUsize,
    sync::Arc,
    time::{Duration, Instant, SystemTime},
};

use banto_opcua_server::{
    OpcUaServer, RunningServer, ServerConfig, TagReading, WriteFuture, WriteHandler,
    model::{
        DataType, MappedValue, Quality, SampleTimestamps, WritePrincipal, WriteRejection,
        WriteRequest,
    },
};
use opcua::{
    client::{ClientBuilder, DataChangeCallback, IdentityToken, Session},
    crypto::SecurityPolicy,
    types::{
        AttributeId, DataValue, DateTime, MessageSecurityMode, MonitoredItemCreateRequest, NodeId,
        ReadValueId, StatusCode, TimestampsToReturn, UserTokenPolicy, Variant, WriteValue,
    },
};
use tokio::sync::{Semaphore, mpsc};

const CONNECTION: &str = "PLC2";
const GROUP: &str = "Cell1";

/// 呼ばれた書き込みを記録し、タグ名で結果を決めるハンドラー。
///
/// 名前が `Slow` で始まるタグは、`gate` に許可が足されるまで結果を返さない。
/// 拒否の理由と同じ名前のタグは、その理由で拒否する。
struct ScriptedHandler {
    calls: mpsc::UnboundedSender<WriteRequest>,
    finished: mpsc::UnboundedSender<String>,
    gate: Arc<Semaphore>,
}

impl WriteHandler for ScriptedHandler {
    fn write(&self, request: WriteRequest) -> WriteFuture {
        self.calls
            .send(request.clone())
            .expect("呼び出しを記録する");
        let finished = self.finished.clone();
        let gate = self.gate.clone();
        Box::pin(async move {
            if request.tag.starts_with("Slow") {
                gate.acquire().await.expect("gate").forget();
            }
            finished.send(request.tag.clone()).expect("完了を記録する");
            match REJECTIONS.iter().find(|(name, ..)| *name == request.tag) {
                Some((_, rejection, _)) => Err(*rejection),
                None => Ok(()),
            }
        })
    }
}

const REJECTIONS: [(&str, WriteRejection, StatusCode); 9] = [
    (
        "TagNotFound",
        WriteRejection::TagNotFound,
        StatusCode::BadNodeIdUnknown,
    ),
    (
        "NotWritable",
        WriteRejection::NotWritable,
        StatusCode::BadNotWritable,
    ),
    (
        "AccessDenied",
        WriteRejection::AccessDenied,
        StatusCode::BadUserAccessDenied,
    ),
    (
        "TypeMismatch",
        WriteRejection::TypeMismatch,
        StatusCode::BadTypeMismatch,
    ),
    (
        "OutOfRange",
        WriteRejection::OutOfRange,
        StatusCode::BadOutOfRange,
    ),
    (
        "OutOfService",
        WriteRejection::OutOfService,
        StatusCode::BadOutOfService,
    ),
    (
        "RateLimited",
        WriteRejection::RateLimited,
        StatusCode::BadRequestNotAllowed,
    ),
    (
        "CommunicationError",
        WriteRejection::CommunicationError,
        StatusCode::BadCommunicationError,
    ),
    (
        "Internal",
        WriteRejection::Internal,
        StatusCode::BadInternalError,
    ),
];

struct Harness {
    server: RunningServer,
    calls: mpsc::UnboundedReceiver<WriteRequest>,
    finished: mpsc::UnboundedReceiver<String>,
    gate: Arc<Semaphore>,
}

impl Harness {
    async fn start(config: ServerConfig) -> Self {
        let (calls_tx, calls) = mpsc::unbounded_channel();
        let (finished_tx, finished) = mpsc::unbounded_channel();
        let gate = Arc::new(Semaphore::new(0));
        let server = OpcUaServer::start_with_write_handler(
            config.with_port(0),
            Arc::new(ScriptedHandler {
                calls: calls_tx,
                finished: finished_tx,
                gate: gate.clone(),
            }),
        )
        .await
        .expect("サーバーを起動する");
        Self {
            server,
            calls,
            finished,
            gate,
        }
    }

    /// 書き込み可の Double タグを登録して NodeId を返す。
    fn writable(&self, tag: &str) -> NodeId {
        self.server
            .add_tag(CONNECTION, GROUP, tag, MappedValue::Double(1.0))
            .expect("タグを登録する");
        self.server
            .set_tag_writable(CONNECTION, GROUP, tag, true)
            .expect("書き込み可にする");
        tag_id(&self.server, tag)
    }

    fn assert_no_calls(&mut self) {
        assert!(self.calls.try_recv().is_err(), "ハンドラーは呼ばれていない");
    }
}

fn tag_id(server: &RunningServer, tag: &str) -> NodeId {
    NodeId::new(
        server.namespace_index(),
        format!("{CONNECTION}.{GROUP}.{tag}"),
    )
}

async fn connect(server: &RunningServer) -> (Arc<Session>, tempfile::TempDir) {
    let pki = tempfile::tempdir().expect("一時 PKI を作る");
    let mut client = ClientBuilder::new()
        .application_name("banto-opcua-write-test")
        .application_uri("urn:banto:opcua:write-test")
        .session_name("write-test-session")
        .create_sample_keypair(true)
        .pki_dir(pki.path())
        .trust_server_certs(true)
        .verify_server_certs(false)
        .client()
        .expect("クライアントを作る");
    let (session, event_loop) = client
        .connect_to_matching_endpoint(
            (
                server.endpoint(),
                SecurityPolicy::None.to_str(),
                MessageSecurityMode::None,
                UserTokenPolicy::anonymous(),
            ),
            IdentityToken::Anonymous,
        )
        .await
        .expect("エンドポイントに接続する");
    event_loop.spawn();
    assert!(
        tokio::time::timeout(Duration::from_secs(10), session.wait_for_connection())
            .await
            .expect("接続待ちが時間切れ")
    );
    (session, pki)
}

fn value_write(id: &NodeId, value: DataValue) -> WriteValue {
    WriteValue {
        node_id: id.clone(),
        attribute_id: AttributeId::Value as u32,
        index_range: Default::default(),
        value,
    }
}

fn double(number: f64) -> DataValue {
    DataValue {
        value: Some(Variant::Double(number)),
        ..Default::default()
    }
}

async fn write_one(session: &Session, id: &NodeId, value: DataValue) -> StatusCode {
    let results = session
        .write(&[value_write(id, value)])
        .await
        .expect("Write する");
    assert_eq!(results.len(), 1);
    results[0]
}

async fn read_attribute(session: &Session, id: &NodeId, attribute: AttributeId) -> DataValue {
    session
        .read(
            &[ReadValueId {
                node_id: id.clone(),
                attribute_id: attribute as u32,
                ..Default::default()
            }],
            TimestampsToReturn::Both,
            0.0,
        )
        .await
        .expect("Read する")
        .remove(0)
}

async fn read_value(session: &Session, id: &NodeId) -> Option<Variant> {
    read_attribute(session, id, AttributeId::Value).await.value
}

async fn recv<T>(receiver: &mut mpsc::UnboundedReceiver<T>) -> T {
    tokio::time::timeout(Duration::from_secs(10), receiver.recv())
        .await
        .expect("時間切れ")
        .expect("送り手が残っている")
}

fn reading(number: f64) -> TagReading {
    let now = SystemTime::now();
    TagReading::new(
        DataType::Double,
        Some(number),
        Quality::Good,
        SampleTimestamps::new(Some(now), now),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn handler_results_map_to_status_codes() {
    let mut harness = Harness::start(ServerConfig::default()).await;
    let (session, _pki) = connect(&harness.server).await;

    let ok = harness.writable("Ok");
    // asyncua と同じく SourceTimestamp を付けて書く。時刻は受け付けて捨てる。
    let timed = DataValue {
        value: Some(Variant::Double(5.0)),
        source_timestamp: Some(DateTime::now()),
        server_timestamp: Some(DateTime::now()),
        ..Default::default()
    };
    assert_eq!(write_one(&session, &ok, timed).await, StatusCode::Good);
    let call = recv(&mut harness.calls).await;
    assert_eq!(
        (
            call.connection.as_str(),
            call.group.as_str(),
            call.tag.as_str()
        ),
        (CONNECTION, GROUP, "Ok")
    );
    assert_eq!(call.value, MappedValue::Double(5.0));
    assert_eq!(call.principal, WritePrincipal::Anonymous);
    assert_eq!(call.client.application_uri, "urn:banto:opcua:write-test");
    assert_eq!(call.client.application_name, "banto-opcua-write-test");
    assert_eq!(call.client.session_name, "write-test-session");
    assert!(!call.client.session_id.is_empty());
    assert!(
        call.client.endpoint_url.starts_with("opc.tcp://"),
        "{}",
        call.client.endpoint_url
    );

    for (name, _, expected) in REJECTIONS {
        let id = harness.writable(name);
        assert_eq!(
            write_one(&session, &id, double(2.0)).await,
            expected,
            "{name}"
        );
        assert_eq!(recv(&mut harness.calls).await.tag, name);
    }

    // Boolean のタグにも書ける。
    harness
        .server
        .add_tag(CONNECTION, GROUP, "Run", MappedValue::Boolean(false))
        .expect("タグを登録する");
    harness
        .server
        .set_tag_writable(CONNECTION, GROUP, "Run", true)
        .expect("書き込み可にする");
    let run = tag_id(&harness.server, "Run");
    let boolean = DataValue {
        value: Some(Variant::Boolean(true)),
        ..Default::default()
    };
    assert_eq!(write_one(&session, &run, boolean).await, StatusCode::Good);
    assert_eq!(
        recv(&mut harness.calls).await.value,
        MappedValue::Boolean(true)
    );
    harness.server.shutdown().await.expect("サーバーを停止する");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_server_rejects_before_calling_the_handler() {
    let mut harness = Harness::start(ServerConfig::default()).await;
    let (session, _pki) = connect(&harness.server).await;
    let speed = harness.writable("Speed");

    let rejected = [
        // Double のノードへ Boolean。
        (
            value_write(
                &speed,
                DataValue {
                    value: Some(Variant::Boolean(true)),
                    ..Default::default()
                },
            ),
            StatusCode::BadTypeMismatch,
        ),
        // Double のノードへ Int32（型の変換はしない）。
        (
            value_write(
                &speed,
                DataValue {
                    value: Some(Variant::Int32(2)),
                    ..Default::default()
                },
            ),
            StatusCode::BadTypeMismatch,
        ),
        // 非有限の Double。
        (
            value_write(&speed, double(f64::NAN)),
            StatusCode::BadOutOfRange,
        ),
        // IndexRange 付き。
        (
            WriteValue {
                index_range: "0".parse().expect("IndexRange"),
                ..value_write(&speed, double(2.0))
            },
            StatusCode::BadIndexRangeNoData,
        ),
        // Bad / Uncertain の StatusCode 付き。
        (
            value_write(
                &speed,
                DataValue {
                    status: Some(StatusCode::Bad),
                    ..double(2.0)
                },
            ),
            StatusCode::BadWriteNotSupported,
        ),
        (
            value_write(
                &speed,
                DataValue {
                    status: Some(StatusCode::UncertainLastUsableValue),
                    ..double(2.0)
                },
            ),
            StatusCode::BadWriteNotSupported,
        ),
        // Value 以外の属性。
        (
            WriteValue {
                attribute_id: AttributeId::DisplayName as u32,
                ..value_write(&speed, double(2.0))
            },
            StatusCode::BadNotWritable,
        ),
        // AccessLevel が CurrentRead のみのサンプルタグ。
        (
            value_write(
                &NodeId::new(harness.server.namespace_index(), "PLC1.Line1.Speed"),
                double(2.0),
            ),
            StatusCode::BadNotWritable,
        ),
    ];
    let (requests, expected): (Vec<_>, Vec<_>) = rejected.into_iter().unzip();
    assert_eq!(
        session.write(&requests).await.expect("Write する"),
        expected
    );
    harness.assert_no_calls();
    harness.server.shutdown().await.expect("サーバーを停止する");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tags_that_are_not_writable_stay_current_read() {
    let mut harness = Harness::start(ServerConfig::default()).await;
    let (session, _pki) = connect(&harness.server).await;
    harness
        .server
        .add_tag(CONNECTION, GROUP, "Speed", MappedValue::Double(1.0))
        .expect("タグを登録する");
    let speed = tag_id(&harness.server, "Speed");
    let current_read = Some(Variant::Byte(1));
    let read_write = Some(Variant::Byte(3));

    for attribute in [AttributeId::AccessLevel, AttributeId::UserAccessLevel] {
        assert_eq!(
            read_attribute(&session, &speed, attribute).await.value,
            current_read
        );
    }
    assert_eq!(
        write_one(&session, &speed, double(2.0)).await,
        StatusCode::BadNotWritable
    );

    harness
        .server
        .set_tag_writable(CONNECTION, GROUP, "Speed", true)
        .expect("書き込み可にする");
    for attribute in [AttributeId::AccessLevel, AttributeId::UserAccessLevel] {
        assert_eq!(
            read_attribute(&session, &speed, attribute).await.value,
            read_write
        );
    }
    harness
        .server
        .set_tag_writable(CONNECTION, GROUP, "Speed", false)
        .expect("書き込み不可に戻す");
    assert_eq!(
        write_one(&session, &speed, double(2.0)).await,
        StatusCode::BadNotWritable
    );

    // 削除して再登録したタグは、書き込み不可から始まる。
    harness
        .server
        .set_tag_writable(CONNECTION, GROUP, "Speed", true)
        .expect("書き込み可にする");
    harness
        .server
        .remove_tag(CONNECTION, GROUP, "Speed")
        .expect("タグを削除する");
    harness
        .server
        .add_tag(CONNECTION, GROUP, "Speed", MappedValue::Double(1.0))
        .expect("タグを再登録する");
    assert_eq!(
        read_attribute(&session, &speed, AttributeId::AccessLevel)
            .await
            .value,
        current_read
    );
    assert_eq!(
        write_one(&session, &speed, double(2.0)).await,
        StatusCode::BadNotWritable
    );

    // サンプルタグと、登録していないタグは切り替えられない。
    assert!(
        harness
            .server
            .set_tag_writable("PLC1", "Line1", "Speed", true)
            .is_err()
    );
    assert!(
        harness
            .server
            .set_tag_writable(CONNECTION, GROUP, "Missing", true)
            .is_err()
    );
    harness.assert_no_calls();
    harness.server.shutdown().await.expect("サーバーを停止する");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn without_a_handler_every_write_is_not_writable() {
    let server = OpcUaServer::start(ServerConfig::default().with_port(0))
        .await
        .expect("サーバーを起動する");
    server
        .add_tag(CONNECTION, GROUP, "Speed", MappedValue::Double(1.0))
        .expect("タグを登録する");
    server
        .set_tag_writable(CONNECTION, GROUP, "Speed", true)
        .expect("ハンドラーが無くても切り替えはエラーにしない");
    let speed = tag_id(&server, "Speed");
    let (session, _pki) = connect(&server).await;
    assert_eq!(
        read_attribute(&session, &speed, AttributeId::AccessLevel)
            .await
            .value,
        Some(Variant::Byte(1))
    );
    assert_eq!(
        write_one(&session, &speed, double(2.0)).await,
        StatusCode::BadNotWritable
    );
    server.shutdown().await.expect("サーバーを停止する");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_slow_handler_does_not_block_reads_or_subscriptions() {
    let mut harness = Harness::start(ServerConfig::default()).await;
    let (session, _pki) = connect(&harness.server).await;
    let slow = harness.writable("Slow");

    let (sender, mut changes) = mpsc::unbounded_channel();
    let subscription = session
        .create_subscription(
            Duration::from_millis(50),
            30,
            10,
            0,
            0,
            true,
            DataChangeCallback::new(move |value, _item| {
                let _ = sender.send(value);
            }),
        )
        .await
        .expect("購読を作る");
    let request: MonitoredItemCreateRequest = slow.clone().into();
    session
        .create_monitored_items(subscription, TimestampsToReturn::Both, vec![request])
        .await
        .expect("監視項目を作る");
    assert_eq!(recv(&mut changes).await.value, Some(Variant::Double(1.0)));

    let writer = {
        let session = session.clone();
        let slow = slow.clone();
        tokio::spawn(async move { write_one(&session, &slow, double(5.0)).await })
    };
    assert_eq!(recv(&mut harness.calls).await.tag, "Slow");

    // ハンドラーが待っている間も、Read と購読は止まらない。
    let started = Instant::now();
    assert_eq!(
        read_value(&session, &slow).await,
        Some(Variant::Double(1.0))
    );
    harness
        .server
        .update_tag_reading(CONNECTION, GROUP, "Slow", reading(3.0))
        .expect("値を更新する");
    assert_eq!(recv(&mut changes).await.value, Some(Variant::Double(3.0)));
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "Read と購読はハンドラーを待たない"
    );
    assert!(!writer.is_finished(), "Write はハンドラーの結果を待つ");

    harness.gate.add_permits(1);
    assert_eq!(writer.await.expect("書き込みタスク"), StatusCode::Good);
    // Write の成功だけでは値は変わらず、update_tag_reading で変わる。
    assert_eq!(
        read_value(&session, &slow).await,
        Some(Variant::Double(3.0))
    );
    harness
        .server
        .update_tag_reading(CONNECTION, GROUP, "Slow", reading(5.0))
        .expect("機器から戻った値を反映する");
    assert_eq!(
        read_value(&session, &slow).await,
        Some(Variant::Double(5.0))
    );
    harness.server.shutdown().await.expect("サーバーを停止する");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn multi_node_writes_report_each_node_and_respect_the_node_limit() {
    let mut harness = Harness::start(
        ServerConfig::default().with_max_nodes_per_write(NonZeroUsize::new(3).expect("3")),
    )
    .await;
    let (session, _pki) = connect(&harness.server).await;
    let ok = harness.writable("Ok");
    let limited = harness.writable("RateLimited");
    harness
        .server
        .add_tag(CONNECTION, GROUP, "ReadOnly", MappedValue::Double(1.0))
        .expect("タグを登録する");
    let read_only = tag_id(&harness.server, "ReadOnly");

    let requests = [
        value_write(&ok, double(1.0)),
        value_write(&read_only, double(1.0)),
        value_write(&limited, double(1.0)),
    ];
    assert_eq!(
        session.write(&requests).await.expect("Write する"),
        vec![
            StatusCode::Good,
            StatusCode::BadNotWritable,
            StatusCode::BadRequestNotAllowed,
        ]
    );
    // 要求の順に 1 件ずつ呼ぶ。
    assert_eq!(recv(&mut harness.calls).await.tag, "Ok");
    assert_eq!(recv(&mut harness.calls).await.tag, "RateLimited");

    let too_many = vec![value_write(&ok, double(1.0)); 4];
    assert_eq!(
        session
            .write(&too_many)
            .await
            .expect_err("上限を超える")
            .status(),
        StatusCode::BadTooManyOperations
    );
    harness.assert_no_calls();
    harness.server.shutdown().await.expect("サーバーを停止する");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn closing_the_session_finishes_the_running_node_and_skips_the_rest() {
    let mut harness = Harness::start(ServerConfig::default()).await;
    let (session, _pki) = connect(&harness.server).await;
    let first = harness.writable("Slow1");
    let second = harness.writable("Slow2");

    let writer = {
        let session = session.clone();
        let requests = [
            value_write(&first, double(1.0)),
            value_write(&second, double(1.0)),
        ];
        tokio::spawn(async move { session.write(&requests).await })
    };
    assert_eq!(recv(&mut harness.calls).await.tag, "Slow1");
    session.disconnect().await.expect("切断する");

    // 走っている 1 件は最後まで終わり、残りのノードではハンドラーを呼ばない。
    harness.gate.add_permits(2);
    assert_eq!(recv(&mut harness.finished).await, "Slow1");
    let _ = tokio::time::timeout(Duration::from_secs(10), writer).await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    harness.assert_no_calls();
    harness.server.shutdown().await.expect("サーバーを停止する");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn writes_beyond_the_concurrency_limit_are_resource_unavailable() {
    let mut harness = Harness::start(
        ServerConfig::default().with_max_concurrent_writes(NonZeroUsize::new(1).expect("1")),
    )
    .await;
    let (session, _pki) = connect(&harness.server).await;
    let slow = harness.writable("Slow");
    let ok = harness.writable("Ok");

    let writer = {
        let session = session.clone();
        let slow = slow.clone();
        tokio::spawn(async move { write_one(&session, &slow, double(1.0)).await })
    };
    assert_eq!(recv(&mut harness.calls).await.tag, "Slow");
    assert_eq!(
        write_one(&session, &ok, double(1.0)).await,
        StatusCode::BadResourceUnavailable
    );
    harness.assert_no_calls();

    harness.gate.add_permits(1);
    assert_eq!(writer.await.expect("書き込みタスク"), StatusCode::Good);
    // 終わったハンドラーの枠は返る。
    assert_eq!(
        write_one(&session, &ok, double(1.0)).await,
        StatusCode::Good
    );
    assert_eq!(recv(&mut harness.calls).await.tag, "Ok");
    harness.server.shutdown().await.expect("サーバーを停止する");
}
