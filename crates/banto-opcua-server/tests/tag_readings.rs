//! 実タグの未取得・品質変更・時刻を OPC UA の実接続で確認する。

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use banto_opcua_server::{
    OpcUaServer, ServerConfig, TagReading,
    model::{DataType, Quality, SampleTimestamps},
};
use opcua::{
    client::{ClientBuilder, DataChangeCallback, IdentityToken, Session},
    crypto::SecurityPolicy,
    types::{
        AttributeId, DataTypeId, DataValue, DateTime, MessageSecurityMode,
        MonitoredItemCreateRequest, NodeId, ReadValueId, StatusCode, TimestampsToReturn,
        UserTokenPolicy, Variant, WriteValue,
    },
};
use tokio::sync::mpsc;

fn at(milliseconds: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(1_700_000_000) + Duration::from_millis(milliseconds)
}

fn reading(
    data_type: DataType,
    value: Option<f64>,
    quality: Quality,
    sampled_ms: Option<u64>,
    quality_ms: Option<u64>,
    server_ms: u64,
) -> TagReading {
    TagReading::new(
        data_type,
        value,
        quality,
        SampleTimestamps::for_quality(
            quality,
            sampled_ms.map(at),
            quality_ms.map(at),
            at(server_ms),
        ),
    )
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dynamic_readings_keep_type_quality_and_timestamps_through_subscription() {
    let server = OpcUaServer::start(ServerConfig::default().with_port(0).with_sample_tags(false))
        .await
        .expect("サーバーを起動する");
    let missing = reading(DataType::Double, None, Quality::Bad, None, None, 10);
    server
        .add_tag_reading("PLC1", "Line1", "Speed", missing)
        .expect("固定タグを置かず実タグを登録する");
    server
        .add_tag_reading(
            "PLC1",
            "Line1",
            "Running",
            reading(
                DataType::Boolean,
                Some(0.0),
                Quality::Good,
                Some(10),
                None,
                20,
            ),
        )
        .expect("Boolean を登録する");

    let pki = tempfile::tempdir().expect("一時 PKI を作る");
    let mut client = ClientBuilder::new()
        .application_name("banto-opcua-reading-test")
        .application_uri("urn:banto:opcua:reading-test")
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
        .expect("接続する");
    let task = event_loop.spawn();
    assert!(
        tokio::time::timeout(Duration::from_secs(10), session.wait_for_connection())
            .await
            .expect("接続待ちが時間切れ")
    );

    let ns = server.namespace_index();
    let speed = NodeId::new(ns, "PLC1.Line1.Speed");
    let running = NodeId::new(ns, "PLC1.Line1.Running");
    assert_eq!(
        read_attribute(&session, &speed, AttributeId::DataType)
            .await
            .value,
        Some(Variant::NodeId(Box::new(DataTypeId::Double.into())))
    );
    assert_eq!(
        read_attribute(&session, &running, AttributeId::DataType)
            .await
            .value,
        Some(Variant::NodeId(Box::new(DataTypeId::Boolean.into())))
    );
    let first = read_attribute(&session, &speed, AttributeId::Value).await;
    assert_reading(&first, Some(Variant::Empty), StatusCode::Bad, None, 10);
    let bit = read_attribute(&session, &running, AttributeId::Value).await;
    assert_reading(
        &bit,
        Some(Variant::Boolean(false)),
        StatusCode::Good,
        Some(10),
        20,
    );

    let writes = session
        .write(&[WriteValue::value_attr(speed.clone(), Variant::Double(99.0))])
        .await
        .expect("Write 応答を受ける");
    assert!(matches!(
        writes.as_slice(),
        [StatusCode::BadNotWritable | StatusCode::BadUserAccessDenied]
    ));

    let (sender, mut receiver) = mpsc::unbounded_channel();
    let subscription = session
        .create_subscription(
            Duration::from_millis(100),
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
    let mut request: MonitoredItemCreateRequest = speed.clone().into();
    request.requested_parameters.sampling_interval = 0.0;
    let items = session
        .create_monitored_items(subscription, TimestampsToReturn::Both, vec![request])
        .await
        .expect("監視項目を作る");
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].result.revised_sampling_interval, 0.0);
    assert_change(
        &mut receiver,
        Some(Variant::Empty),
        StatusCode::Bad,
        None,
        10,
    )
    .await;

    let good = reading(
        DataType::Double,
        Some(12.5),
        Quality::Good,
        Some(100),
        None,
        110,
    );
    server
        .update_tag_reading("PLC1", "Line1", "Speed", good)
        .expect("Good に復旧する");
    assert_change(
        &mut receiver,
        Some(Variant::Double(12.5)),
        StatusCode::Good,
        Some(100),
        110,
    )
    .await;
    let repeated = read_attribute(&session, &speed, AttributeId::Value).await;
    assert_reading(
        &repeated,
        Some(Variant::Double(12.5)),
        StatusCode::Good,
        Some(100),
        110,
    );
    assert_eq!(
        read_attribute(&session, &speed, AttributeId::Value)
            .await
            .source_timestamp,
        repeated.source_timestamp
    );

    server
        .update_tag_reading(
            "PLC1",
            "Line1",
            "Speed",
            reading(
                DataType::Double,
                Some(12.75),
                Quality::Good,
                Some(150),
                None,
                160,
            ),
        )
        .expect("50 ms 後の採取値を反映する");
    assert_change(
        &mut receiver,
        Some(Variant::Double(12.75)),
        StatusCode::Good,
        Some(150),
        160,
    )
    .await;
    assert_reading(
        &read_attribute(&session, &speed, AttributeId::Value).await,
        Some(Variant::Double(12.75)),
        StatusCode::Good,
        Some(150),
        160,
    );

    server
        .update_tag_reading(
            "PLC1",
            "Line1",
            "Speed",
            reading(
                DataType::Double,
                Some(12.5),
                Quality::Stale,
                Some(100),
                Some(200),
                210,
            ),
        )
        .expect("同じ値を Stale にする");
    assert_change(
        &mut receiver,
        Some(Variant::Double(12.5)),
        StatusCode::UncertainLastUsableValue,
        Some(200),
        210,
    )
    .await;

    server
        .update_tag_reading(
            "PLC1",
            "Line1",
            "Speed",
            reading(DataType::Double, None, Quality::Bad, None, Some(300), 310),
        )
        .expect("Bad にする");
    assert_change(
        &mut receiver,
        Some(Variant::Empty),
        StatusCode::Bad,
        Some(300),
        310,
    )
    .await;

    assert!(
        server
            .update_tag_reading(
                "PLC1",
                "Line1",
                "Speed",
                reading(
                    DataType::Boolean,
                    Some(1.0),
                    Quality::Good,
                    Some(400),
                    None,
                    410
                ),
            )
            .is_err(),
        "既存ノードの DataType を黙って変えない"
    );
    assert_eq!(
        read_attribute(&session, &speed, AttributeId::Value)
            .await
            .status(),
        StatusCode::Bad
    );
    server
        .update_tag_reading(
            "PLC1",
            "Line1",
            "Speed",
            reading(
                DataType::Double,
                Some(12.5),
                Quality::Good,
                Some(400),
                None,
                410,
            ),
        )
        .expect("Bad から復旧する");
    assert_change(
        &mut receiver,
        Some(Variant::Double(12.5)),
        StatusCode::Good,
        Some(400),
        410,
    )
    .await;

    server
        .update_tag_reading(
            "PLC1",
            "Line1",
            "Speed",
            reading(
                DataType::Double,
                Some(f64::NAN),
                Quality::Good,
                Some(500),
                None,
                510,
            ),
        )
        .expect("非有限値を Bad に写す");
    assert_reading(
        &read_attribute(&session, &speed, AttributeId::Value).await,
        Some(Variant::Empty),
        StatusCode::Bad,
        Some(500),
        510,
    );
    server
        .update_tag_reading(
            "PLC1",
            "Line1",
            "Running",
            reading(
                DataType::Boolean,
                Some(2.0),
                Quality::Good,
                Some(600),
                None,
                610,
            ),
        )
        .expect("不正な bit を Bad に写す");
    assert_reading(
        &read_attribute(&session, &running, AttributeId::Value).await,
        Some(Variant::Empty),
        StatusCode::Bad,
        Some(600),
        610,
    );

    session.disconnect().await.expect("切断する");
    task.abort();
    server.shutdown().await.expect("停止する");
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
        .expect("Read に成功する")
        .into_iter()
        .next()
        .expect("Read 結果がある")
}

fn assert_reading(
    reading: &DataValue,
    value: Option<Variant>,
    status: StatusCode,
    source_ms: Option<u64>,
    server_ms: u64,
) {
    assert_eq!(reading.value, value);
    assert_eq!(reading.status(), status);
    assert_eq!(
        reading.source_timestamp,
        source_ms.map(|ms| DateTime::from(
            banto_opcua_core::opcua_ticks_from_system_time(at(ms)).unwrap()
        ))
    );
    assert_eq!(
        reading.server_timestamp,
        Some(DateTime::from(
            banto_opcua_core::opcua_ticks_from_system_time(at(server_ms)).unwrap()
        ))
    );
}

async fn assert_change(
    receiver: &mut mpsc::UnboundedReceiver<DataValue>,
    value: Option<Variant>,
    status: StatusCode,
    source_ms: Option<u64>,
    server_ms: u64,
) {
    let change = tokio::time::timeout(Duration::from_secs(5), receiver.recv())
        .await
        .expect("DataChange の待ち時間が切れた")
        .expect("DataChange が届く");
    assert_reading(&change, value, status, source_ms, server_ms);
}
