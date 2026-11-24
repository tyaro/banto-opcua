//! 元の型のまま公開する値（ADR-0007、#56）を、本リポジトリのクライアントの部品で確かめる。

use std::{sync::Arc, time::SystemTime};

use banto_opcua_client::{
    ClientConfig, ConnectedClient, OpcUaClient,
    model::{NodeAddress, ScalarValue, StatusCode as ClientStatus},
};
use banto_opcua_server::{
    OpcUaServer, RunningServer, ServerConfig, TagReading, WriteFuture, WriteHandler,
    model::{DataType, MAX_STRING_BYTES, MappedValue, Quality, SampleTimestamps, WriteRequest},
};
use tokio::sync::mpsc;

const TAGS_URI: &str = "urn:banto:opcua:tags";
const CONNECTION: &str = "PLC3";
const GROUP: &str = "Typed";

/// 受けた書き込みの値を記録し、すべて受け付ける。
struct Recorder(mpsc::UnboundedSender<(String, MappedValue)>);

impl WriteHandler for Recorder {
    fn write(&self, request: WriteRequest) -> WriteFuture {
        self.0
            .send((request.tag, request.value))
            .expect("書き込みを記録する");
        Box::pin(async { Ok(()) })
    }
}

/// タグ名、型、公開する値。64 ビットの整数は 2^53 を超える値で精度を確かめる。
fn typed_tags() -> Vec<(&'static str, MappedValue)> {
    vec![
        ("I16", MappedValue::Int16(i16::MIN)),
        ("U16", MappedValue::UInt16(u16::MAX)),
        ("I32", MappedValue::Int32(i32::MIN)),
        ("U32", MappedValue::UInt32(u32::MAX)),
        ("I64", MappedValue::Int64(-(1_i64 << 53) - 1)),
        ("U64", MappedValue::UInt64((1_u64 << 53) + 1)),
        ("F32", MappedValue::Float(1.5)),
        ("F64", MappedValue::Double(-2.25)),
        ("Text", MappedValue::String("温度センサー 1".to_owned())),
    ]
}

fn tag(name: &str) -> NodeAddress {
    format!("nsu={TAGS_URI};s={CONNECTION}.{GROUP}.{name}")
        .parse()
        .expect("NodeAddress を解析できる")
}

fn good(value: MappedValue) -> TagReading {
    let now = SystemTime::now();
    TagReading::typed(
        value.data_type(),
        Some(value),
        Quality::Good,
        SampleTimestamps::new(Some(now), now),
    )
}

async fn start() -> (
    RunningServer,
    ConnectedClient,
    mpsc::UnboundedReceiver<(String, MappedValue)>,
) {
    let (calls, received) = mpsc::unbounded_channel();
    let server = OpcUaServer::start_with_write_handler(
        ServerConfig::default()
            .with_none_endpoint(true)
            .with_port(0)
            .with_sample_tags(false),
        Arc::new(Recorder(calls)),
    )
    .await
    .expect("サーバーを起動する");
    for (name, value) in typed_tags() {
        server
            .add_tag_reading(CONNECTION, GROUP, name, good(value))
            .expect("タグを登録する");
        server
            .set_tag_writable(CONNECTION, GROUP, name, true)
            .expect("書き込み可にする");
    }
    let client = OpcUaClient::connect_with(ClientConfig::new(server.endpoint()))
        .await
        .expect("クライアントが接続する");
    (server, client, received)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn each_type_is_read_back_as_published() {
    let (server, client, _received) = start().await;
    let tags = typed_tags();
    let nodes: Vec<_> = tags.iter().map(|(name, _)| tag(name)).collect();

    let readings = client.read_batch(&nodes).await.expect("まとめて読む");

    for ((name, value), reading) in tags.into_iter().zip(readings) {
        assert_eq!(reading.status, ClientStatus::GOOD, "{name}");
        assert_eq!(reading.value, Some(ScalarValue::from(value)), "{name}");
    }
    drop(client);
    server.shutdown().await.expect("止める");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn invalid_typed_values_are_published_as_bad_without_a_value() {
    let (server, client, _received) = start().await;
    let now = SystemTime::now();
    for (name, value) in [
        ("F32", MappedValue::Float(f32::NAN)),
        ("I64", MappedValue::UInt64(1)),
        (
            "Text",
            MappedValue::String("a".repeat(MAX_STRING_BYTES + 1)),
        ),
    ] {
        let data_type = typed_tags()
            .into_iter()
            .find(|(tag, _)| *tag == name)
            .map(|(_, value)| value.data_type())
            .expect("登録したタグ");
        server
            .update_tag_reading(
                CONNECTION,
                GROUP,
                name,
                TagReading::typed(
                    data_type,
                    Some(value),
                    Quality::Good,
                    SampleTimestamps::new(Some(now), now),
                ),
            )
            .expect("更新する");
    }
    // F32 は下で f64 の 0.1 に上書きするので、NaN の Bad をここで読んでおく。
    let nan = client
        .read_batch(&[tag("F32")])
        .await
        .expect("読む")
        .remove(0);
    assert_eq!(nan.status, ClientStatus::BAD);
    assert_eq!(nan.value, None);

    // f64 からの変換は丸めない（小数の整数、f32 で表せない値）。
    for (name, data_type, number) in [
        ("I16", DataType::Int16, 1.5),
        ("U32", DataType::UInt32, -1.0),
        ("F32", DataType::Float, 0.1),
    ] {
        server
            .update_tag_reading(
                CONNECTION,
                GROUP,
                name,
                TagReading::new(
                    data_type,
                    Some(number),
                    Quality::Good,
                    SampleTimestamps::new(Some(now), now),
                ),
            )
            .expect("更新する");
    }
    server
        .update_tag_reading(
            CONNECTION,
            GROUP,
            "F64",
            TagReading::new(
                DataType::Double,
                Some(0.1),
                Quality::Good,
                SampleTimestamps::new(Some(now), now),
            ),
        )
        .expect("Double の 0.1 はそのまま");

    let readings = client
        .read_batch(&[tag("F32"), tag("I64"), tag("Text"), tag("I16"), tag("U32")])
        .await
        .expect("まとめて読む");
    for reading in readings {
        assert_eq!(reading.status, ClientStatus::BAD);
        assert_eq!(reading.value, None);
    }
    let double = client
        .read_batch(&[tag("F64")])
        .await
        .expect("読む")
        .remove(0);
    assert_eq!(double.value, Some(ScalarValue::Double(0.1)));
    drop(client);
    server.shutdown().await.expect("止める");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn writes_pass_typed_values_and_reject_other_types_or_invalid_values() {
    let (server, client, mut received) = start().await;

    let statuses = client
        .write_batch(&[
            (tag("U64"), ScalarValue::UInt64(u64::MAX)),
            (tag("I16"), ScalarValue::Int16(-2)),
            (tag("Text"), ScalarValue::String("停止".to_owned())),
            (tag("F32"), ScalarValue::Float(0.25)),
            (tag("I32"), ScalarValue::Int16(1)),
            (tag("U16"), ScalarValue::Byte(1)),
            (tag("F32"), ScalarValue::Float(f32::INFINITY)),
            (
                tag("Text"),
                ScalarValue::String("a".repeat(MAX_STRING_BYTES + 1)),
            ),
        ])
        .await
        .expect("まとめて書く");

    assert_eq!(
        statuses,
        vec![
            ClientStatus::GOOD,
            ClientStatus::GOOD,
            ClientStatus::GOOD,
            ClientStatus::GOOD,
            ClientStatus::BAD_TYPE_MISMATCH,
            ClientStatus::BAD_TYPE_MISMATCH,
            ClientStatus::BAD_OUT_OF_RANGE,
            ClientStatus::BAD_OUT_OF_RANGE,
        ]
    );
    let mut calls = Vec::new();
    while let Ok(call) = received.try_recv() {
        calls.push(call);
    }
    calls.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        calls,
        vec![
            ("F32".to_owned(), MappedValue::Float(0.25)),
            ("I16".to_owned(), MappedValue::Int16(-2)),
            ("Text".to_owned(), MappedValue::String("停止".to_owned())),
            ("U64".to_owned(), MappedValue::UInt64(u64::MAX)),
        ],
        "サーバーの検査を通った値だけが、型のままハンドラーへ届く"
    );
    drop(client);
    server.shutdown().await.expect("止める");
}
