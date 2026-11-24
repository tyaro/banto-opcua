//! タグの説明・単位・レンジ・二値の文言（ADR-0008、#55）を OPC UA の実接続で確かめる。

use std::time::{Duration, SystemTime};

use banto_opcua_server::{
    Error, OpcUaServer, RunningServer, ServerConfig, TagReading,
    model::{
        DataType, EngineeringUnits, EuRange, Quality, SampleTimestamps, TagMetadata,
        TwoStateLabels, UNECE_UNITS_NAMESPACE_URI,
    },
};
use opcua::{
    client::{ClientBuilder, DataChangeCallback, IdentityToken, Session},
    crypto::SecurityPolicy,
    types::{
        AttributeId, BrowseDescription, BrowseDirection, DataValue, EUInformation, LocalizedText,
        MessageSecurityMode, MonitoredItemCreateRequest, NodeId, Range, ReadValueId,
        ReferenceTypeId, StatusCode, TimestampsToReturn, UserTokenPolicy, VariableTypeId, Variant,
    },
};
use tokio::sync::mpsc;

const CONNECTION: &str = "PLC4";
const GROUP: &str = "Furnace";

fn reading(data_type: DataType, number: f64) -> TagReading {
    let now = SystemTime::now();
    TagReading::new(
        data_type,
        Some(number),
        Quality::Good,
        SampleTimestamps::new(Some(now), now),
    )
}

fn temperature_metadata() -> TagMetadata {
    TagMetadata::new()
        .with_description("炉内温度")
        .with_engineering_units(EngineeringUnits::display("℃").with_unece_code("CEL"))
        .with_eu_range(EuRange::new(0.0, 1200.0))
}

async fn connect(server: &RunningServer) -> (std::sync::Arc<Session>, tempfile::TempDir) {
    connect_to(server.endpoint()).await
}

async fn connect_to(endpoint: &str) -> (std::sync::Arc<Session>, tempfile::TempDir) {
    let pki = tempfile::tempdir().expect("一時 PKI を作る");
    let mut client = ClientBuilder::new()
        .application_name("banto-opcua-metadata-test")
        .application_uri("urn:banto:opcua:metadata-test")
        .create_sample_keypair(true)
        .pki_dir(pki.path())
        .trust_server_certs(true)
        .verify_server_certs(false)
        .client()
        .expect("クライアントを作る");
    let (session, event_loop) = client
        .connect_to_matching_endpoint(
            (
                endpoint,
                SecurityPolicy::None.to_str(),
                MessageSecurityMode::None,
                UserTokenPolicy::anonymous(),
            ),
            IdentityToken::Anonymous,
        )
        .await
        .expect("接続する");
    event_loop.spawn();
    assert!(
        tokio::time::timeout(Duration::from_secs(10), session.wait_for_connection())
            .await
            .expect("接続待ちが時間切れ")
    );
    (session, pki)
}

fn tag_id(server: &RunningServer, tag: &str) -> NodeId {
    NodeId::new(
        server.namespace_index(),
        format!("{CONNECTION}.{GROUP}.{tag}"),
    )
}

async fn references(
    session: &Session,
    node: &NodeId,
    kind: ReferenceTypeId,
) -> Vec<(String, NodeId)> {
    let result = session
        .browse(
            &[BrowseDescription {
                node_id: node.clone(),
                browse_direction: BrowseDirection::Forward,
                reference_type_id: kind.into(),
                include_subtypes: false,
                result_mask: 63,
                ..Default::default()
            }],
            0,
            None,
        )
        .await
        .expect("Browse に成功する");
    assert_eq!(result[0].status_code, StatusCode::Good);
    result[0]
        .references
        .iter()
        .flatten()
        .map(|reference| {
            (
                reference.browse_name.name.to_string(),
                reference.node_id.node_id.clone(),
            )
        })
        .collect()
}

async fn type_definition(session: &Session, node: &NodeId) -> NodeId {
    let found = references(session, node, ReferenceTypeId::HasTypeDefinition).await;
    assert_eq!(found.len(), 1, "型定義は 1 つ");
    found[0].1.clone()
}

async fn properties(session: &Session, node: &NodeId) -> Vec<(String, NodeId)> {
    let mut found = references(session, node, ReferenceTypeId::HasProperty).await;
    found.sort_by(|a, b| a.0.cmp(&b.0));
    found
}

async fn property(session: &Session, node: &NodeId, name: &str) -> NodeId {
    properties(session, node)
        .await
        .into_iter()
        .find(|(found, _)| found == name)
        .unwrap_or_else(|| panic!("プロパティ {name} が無い"))
        .1
}

async fn read(session: &Session, node: &NodeId, attribute: AttributeId) -> DataValue {
    session
        .read(
            &[ReadValueId {
                node_id: node.clone(),
                attribute_id: attribute as u32,
                ..Default::default()
            }],
            TimestampsToReturn::Both,
            0.0,
        )
        .await
        .expect("Read に成功する")
        .remove(0)
}

fn extension<T: Send + Sync + Clone + 'static>(value: &DataValue) -> T {
    match &value.value {
        Some(Variant::ExtensionObject(object)) => {
            object.inner_as::<T>().expect("期待した構造体").clone()
        }
        other => panic!("ExtensionObject ではない: {other:?}"),
    }
}

fn text(value: &DataValue) -> String {
    match &value.value {
        Some(Variant::LocalizedText(text)) => text.text.to_string(),
        other => panic!("LocalizedText ではない: {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn metadata_becomes_description_type_definition_and_properties() {
    let server = OpcUaServer::start(ServerConfig::default().with_port(0).with_sample_tags(false))
        .await
        .expect("サーバーを起動する");
    server
        .add_tag_reading_with_metadata(
            CONNECTION,
            GROUP,
            "Temperature",
            reading(DataType::Double, 850.0),
            &temperature_metadata(),
        )
        .expect("数値のタグを属性付きで登録する");
    server
        .add_tag_reading_with_metadata(
            CONNECTION,
            GROUP,
            "Pressure",
            reading(DataType::Float, 1.5),
            &TagMetadata::new().with_engineering_units(EngineeringUnits::display("bar")),
        )
        .expect("単位だけのタグを登録する");
    server
        .add_tag_reading_with_metadata(
            CONNECTION,
            GROUP,
            "Running",
            reading(DataType::Boolean, 1.0),
            &TagMetadata::new()
                .with_description("運転中")
                .with_two_state(TwoStateLabels::new("運転", "停止")),
        )
        .expect("bit のタグを文言付きで登録する");
    server
        .add_tag_reading(CONNECTION, GROUP, "Plain", reading(DataType::Int32, 1.0))
        .expect("属性の無いタグを登録する");
    let (session, _pki) = connect(&server).await;

    let temperature = tag_id(&server, "Temperature");
    assert_eq!(
        type_definition(&session, &temperature).await,
        NodeId::from(VariableTypeId::AnalogItemType)
    );
    assert_eq!(
        read(&session, &temperature, AttributeId::Description)
            .await
            .value,
        Some(Variant::LocalizedText(Box::new(LocalizedText::from(
            "炉内温度"
        ))))
    );
    let names: Vec<_> = properties(&session, &temperature)
        .await
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    assert_eq!(names, ["EURange", "EngineeringUnits"]);
    let units: EUInformation = extension(
        &read(
            &session,
            &property(&session, &temperature, "EngineeringUnits").await,
            AttributeId::Value,
        )
        .await,
    );
    assert_eq!(units.unit_id, 4_408_652);
    assert_eq!(units.namespace_uri.as_ref(), UNECE_UNITS_NAMESPACE_URI);
    assert_eq!(units.display_name.text.as_ref(), "℃");
    let range: Range = extension(
        &read(
            &session,
            &property(&session, &temperature, "EURange").await,
            AttributeId::Value,
        )
        .await,
    );
    assert_eq!((range.low, range.high), (0.0, 1200.0));
    let units_node = property(&session, &temperature, "EngineeringUnits").await;
    assert_eq!(
        type_definition(&session, &units_node).await,
        NodeId::from(VariableTypeId::PropertyType)
    );

    let pressure = tag_id(&server, "Pressure");
    assert_eq!(
        type_definition(&session, &pressure).await,
        NodeId::from(VariableTypeId::BaseAnalogType)
    );
    let units: EUInformation = extension(
        &read(
            &session,
            &property(&session, &pressure, "EngineeringUnits").await,
            AttributeId::Value,
        )
        .await,
    );
    assert_eq!(units.unit_id, -1);
    assert!(units.namespace_uri.is_null());

    let running = tag_id(&server, "Running");
    assert_eq!(
        type_definition(&session, &running).await,
        NodeId::from(VariableTypeId::TwoStateDiscreteType)
    );
    assert_eq!(
        text(
            &read(
                &session,
                &property(&session, &running, "TrueState").await,
                AttributeId::Value
            )
            .await
        ),
        "運転"
    );
    assert_eq!(
        text(
            &read(
                &session,
                &property(&session, &running, "FalseState").await,
                AttributeId::Value
            )
            .await
        ),
        "停止"
    );

    let plain = tag_id(&server, "Plain");
    assert_eq!(
        type_definition(&session, &plain).await,
        NodeId::from(VariableTypeId::BaseDataVariableType)
    );
    assert!(properties(&session, &plain).await.is_empty());

    server.shutdown().await.expect("止める");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn updates_keep_the_node_id_and_notify_subscribers() {
    let server = OpcUaServer::start(ServerConfig::default().with_port(0).with_sample_tags(false))
        .await
        .expect("サーバーを起動する");
    server
        .add_tag_reading_with_metadata(
            CONNECTION,
            GROUP,
            "Temperature",
            reading(DataType::Double, 850.0),
            &temperature_metadata(),
        )
        .expect("登録する");
    let (session, _pki) = connect(&server).await;
    let temperature = tag_id(&server, "Temperature");
    let range_node = property(&session, &temperature, "EURange").await;

    let (sender, mut changes) = mpsc::unbounded_channel();
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
    let mut request: MonitoredItemCreateRequest = range_node.clone().into();
    request.requested_parameters.sampling_interval = 0.0;
    session
        .create_monitored_items(subscription, TimestampsToReturn::Both, vec![request])
        .await
        .expect("EURange を監視する");
    let first: Range = extension(&recv(&mut changes).await);
    assert_eq!((first.low, first.high), (0.0, 1200.0));

    server
        .update_tag_metadata(
            CONNECTION,
            GROUP,
            "Temperature",
            &TagMetadata::new()
                .with_engineering_units(EngineeringUnits::display("K").with_unece_code("KEL"))
                .with_eu_range(EuRange::new(273.0, 1473.0)),
        )
        .expect("形が同じなら同じ NodeId のまま書き換える");
    let updated: Range = extension(&recv(&mut changes).await);
    assert_eq!((updated.low, updated.high), (273.0, 1473.0));
    assert_eq!(
        property(&session, &temperature, "EURange").await,
        range_node,
        "プロパティの NodeId は変わらない"
    );
    let units: EUInformation = extension(
        &read(
            &session,
            &property(&session, &temperature, "EngineeringUnits").await,
            AttributeId::Value,
        )
        .await,
    );
    assert_eq!(units.display_name.text.as_ref(), "K");
    let description = read(&session, &temperature, AttributeId::Description).await;
    assert!(
        matches!(&description.value, Some(Variant::LocalizedText(text)) if text.text.is_null()),
        "Description を外すと空になる: {description:?}"
    );

    server.shutdown().await.expect("止める");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shape_changes_and_invalid_metadata_are_rejected_and_removal_drops_properties() {
    let server = OpcUaServer::start(ServerConfig::default().with_port(0).with_sample_tags(false))
        .await
        .expect("サーバーを起動する");
    server
        .add_tag_reading_with_metadata(
            CONNECTION,
            GROUP,
            "Temperature",
            reading(DataType::Double, 850.0),
            &temperature_metadata(),
        )
        .expect("登録する");
    let (session, _pki) = connect(&server).await;
    let temperature = tag_id(&server, "Temperature");
    let range_node = property(&session, &temperature, "EURange").await;

    let without_range = TagMetadata::new()
        .with_description("変えない")
        .with_engineering_units(EngineeringUnits::display("℃"));
    assert!(
        server
            .update_tag_metadata(CONNECTION, GROUP, "Temperature", &without_range)
            .is_err(),
        "EURange を外すと形が変わる"
    );
    assert_eq!(
        text(&read(&session, &temperature, AttributeId::Description).await),
        "炉内温度",
        "断ったときは何も変えない"
    );
    assert!(
        server
            .update_tag_metadata(
                CONNECTION,
                GROUP,
                "Temperature",
                &temperature_metadata().with_two_state(TwoStateLabels::new("a", "b")),
            )
            .is_err(),
        "数値のタグに二値の文言は付けられない"
    );
    assert!(
        server
            .add_tag_reading_with_metadata(
                CONNECTION,
                GROUP,
                "Flag",
                reading(DataType::Boolean, 0.0),
                &TagMetadata::new().with_engineering_units(EngineeringUnits::display("℃")),
            )
            .is_err(),
        "bit のタグに単位は付けられない"
    );

    server
        .remove_tag(CONNECTION, GROUP, "Temperature")
        .expect("削除する");
    assert_eq!(
        read(&session, &range_node, AttributeId::Value)
            .await
            .status(),
        StatusCode::BadNodeIdUnknown,
        "プロパティもタグと一緒に消える"
    );
    server
        .add_tag_reading_with_metadata(
            CONNECTION,
            GROUP,
            "Temperature",
            reading(DataType::Double, 850.0),
            &without_range,
        )
        .expect("違う形で登録し直す");
    assert_eq!(
        type_definition(&session, &temperature).await,
        NodeId::from(VariableTypeId::BaseAnalogType)
    );
    let names: Vec<_> = properties(&session, &temperature)
        .await
        .into_iter()
        .map(|(name, _)| name)
        .collect();
    assert_eq!(names, ["EngineeringUnits"]);

    server.shutdown().await.expect("止める");
}

async fn recv(receiver: &mut mpsc::UnboundedReceiver<DataValue>) -> DataValue {
    tokio::time::timeout(Duration::from_secs(5), receiver.recv())
        .await
        .expect("DataChange の待ち時間が切れた")
        .expect("DataChange が届く")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_client_component_reads_back_the_published_metadata() {
    use banto_opcua_client::{
        ClientConfig, OpcUaClient,
        model::{NodeAddress, StatusCode as ClientStatus},
    };

    let server = OpcUaServer::start(ServerConfig::default().with_port(0).with_sample_tags(false))
        .await
        .expect("サーバーを起動する");
    let temperature = temperature_metadata();
    let running = TagMetadata::new().with_two_state(TwoStateLabels::new("運転", "停止"));
    server
        .add_tag_reading_with_metadata(
            CONNECTION,
            GROUP,
            "Temperature",
            reading(DataType::Double, 850.0),
            &temperature,
        )
        .expect("登録する");
    server
        .add_tag_reading_with_metadata(
            CONNECTION,
            GROUP,
            "Running",
            reading(DataType::Boolean, 1.0),
            &running,
        )
        .expect("登録する");
    server
        .add_tag_reading(CONNECTION, GROUP, "Plain", reading(DataType::Int32, 1.0))
        .expect("登録する");
    let client = OpcUaClient::connect_with(ClientConfig::new(server.endpoint()))
        .await
        .expect("クライアントが接続する");
    let address = |tag: &str| -> NodeAddress {
        format!("nsu=urn:banto:opcua:tags;s={CONNECTION}.{GROUP}.{tag}")
            .parse()
            .expect("NodeAddress を解析できる")
    };

    let results = client
        .read_metadata(&[
            address("Temperature"),
            address("Running"),
            address("Plain"),
            address("Missing"),
            "nsu=urn:missing;i=1".parse().expect("解析できる"),
        ])
        .await
        .expect("まとめて読む");

    assert_eq!(results.len(), 5);
    assert_eq!(results[0].status, ClientStatus::GOOD);
    assert_eq!(
        results[0].metadata, temperature,
        "単位の UNECE コードまで往復する"
    );
    assert_eq!(results[1].metadata, running);
    assert_eq!(results[2].status, ClientStatus::GOOD);
    assert_eq!(results[2].metadata, TagMetadata::new());
    assert_eq!(results[3].status, ClientStatus::BAD_NODE_ID_UNKNOWN);
    assert_eq!(results[3].metadata, TagMetadata::new());
    assert_eq!(results[4].status, ClientStatus::BAD_NODE_ID_UNKNOWN);

    client.disconnect().await.expect("切断する");
    server.shutdown().await.expect("止める");
}

/// 監視項目ごとの通知を (NodeId, DataValue) で受ける購読を作る。
async fn subscribe(
    session: &Session,
    nodes: &[NodeId],
) -> mpsc::UnboundedReceiver<(NodeId, DataValue)> {
    let (sender, receiver) = mpsc::unbounded_channel();
    let subscription = session
        .create_subscription(
            Duration::from_millis(50),
            30,
            10,
            0,
            0,
            true,
            DataChangeCallback::new(move |value, item| {
                let _ = sender.send((item.item_to_monitor().node_id.clone(), value));
            }),
        )
        .await
        .expect("購読を作る");
    let requests = nodes
        .iter()
        .map(|node| {
            let mut request: MonitoredItemCreateRequest = node.clone().into();
            request.requested_parameters.sampling_interval = 0.0;
            request
        })
        .collect();
    session
        .create_monitored_items(subscription, TimestampsToReturn::Both, requests)
        .await
        .expect("監視項目を作る");
    receiver
}

async fn next_change(
    receiver: &mut mpsc::UnboundedReceiver<(NodeId, DataValue)>,
) -> (NodeId, DataValue) {
    tokio::time::timeout(Duration::from_secs(5), receiver.recv())
        .await
        .expect("DataChange の待ち時間が切れた")
        .expect("DataChange が届く")
}

async fn assert_quiet(receiver: &mut mpsc::UnboundedReceiver<(NodeId, DataValue)>) {
    if let Ok(Some(change)) =
        tokio::time::timeout(Duration::from_millis(500), receiver.recv()).await
    {
        panic!("通知は来ないはず: {change:?}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn semantic_changes_reach_value_subscribers_with_the_semantics_changed_bit() {
    let server = OpcUaServer::start(ServerConfig::default().with_port(0).with_sample_tags(false))
        .await
        .expect("サーバーを起動する");
    let sampled = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    server
        .add_tag_reading_with_metadata(
            CONNECTION,
            GROUP,
            "Temperature",
            TagReading::new(
                DataType::Double,
                Some(850.0),
                Quality::Good,
                SampleTimestamps::new(Some(sampled), sampled),
            ),
            &temperature_metadata(),
        )
        .expect("登録する");
    let (session, _pki) = connect(&server).await;
    let temperature = tag_id(&server, "Temperature");

    let range_node = property(&session, &temperature, "EURange").await;
    assert_eq!(
        read(&session, &range_node, AttributeId::AccessLevel)
            .await
            .value,
        Some(Variant::Byte(1 | 16)),
        "意味を決めるプロパティは CurrentRead と SemanticChange"
    );

    let mut changes = subscribe(&session, std::slice::from_ref(&temperature)).await;
    let (_, first) = next_change(&mut changes).await;
    assert_eq!(first.value, Some(Variant::Double(850.0)));
    assert!(!first.status().semantics_changed());

    server
        .update_tag_metadata(
            CONNECTION,
            GROUP,
            "Temperature",
            &temperature_metadata().with_description("説明だけ変える"),
        )
        .expect("説明だけ変える");
    assert_quiet(&mut changes).await;

    server
        .update_tag_metadata(
            CONNECTION,
            GROUP,
            "Temperature",
            &TagMetadata::new()
                .with_engineering_units(EngineeringUnits::display("K").with_unece_code("KEL"))
                .with_eu_range(EuRange::new(273.0, 1473.0)),
        )
        .expect("単位とレンジを変える");
    let (_, changed) = next_change(&mut changes).await;
    assert!(changed.status().semantics_changed(), "{changed:?}");
    assert!(changed.status().is_good(), "品質は Good のまま");
    assert_eq!(changed.value, Some(Variant::Double(850.0)), "値はそのまま");
    assert_eq!(
        changed.source_timestamp, first.source_timestamp,
        "採取時刻もそのまま"
    );
    let read_after = read(&session, &temperature, AttributeId::Value).await;
    assert!(
        !read_after.status().semantics_changed(),
        "ビットは通知だけに付け、通常の Read には出さない: {read_after:?}"
    );

    server
        .update_tag_reading(
            CONNECTION,
            GROUP,
            "Temperature",
            reading(DataType::Double, 851.0),
        )
        .expect("値を更新する");
    let (_, next) = next_change(&mut changes).await;
    assert_eq!(next.value, Some(Variant::Double(851.0)));
    assert!(
        !next.status().semantics_changed(),
        "次の値の通知にはビットを付けない"
    );

    let shape_change = server.update_tag_metadata(
        CONNECTION,
        GROUP,
        "Temperature",
        &TagMetadata::new().with_engineering_units(EngineeringUnits::display("K")),
    );
    assert!(
        matches!(shape_change, Err(Error::MetadataShapeChanged(_))),
        "形の変化は専用のエラー: {shape_change:?}"
    );
    let invalid = server.update_tag_metadata(
        CONNECTION,
        GROUP,
        "Temperature",
        &temperature_metadata().with_eu_range(EuRange::new(1.0, 0.0)),
    );
    assert!(
        matches!(invalid, Err(Error::TagUpdate(_))),
        "不正な属性は形の変化と区別する: {invalid:?}"
    );

    server.shutdown().await.expect("止める");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn property_subscriptions_resume_when_the_tag_is_registered_again() {
    let server = OpcUaServer::start(ServerConfig::default().with_port(0).with_sample_tags(false))
        .await
        .expect("サーバーを起動する");
    server
        .add_tag_reading_with_metadata(
            CONNECTION,
            GROUP,
            "Temperature",
            reading(DataType::Double, 850.0),
            &temperature_metadata(),
        )
        .expect("登録する");
    let (session, _pki) = connect(&server).await;
    let temperature = tag_id(&server, "Temperature");
    let range_node = property(&session, &temperature, "EURange").await;
    let units_node = property(&session, &temperature, "EngineeringUnits").await;
    let mut changes = subscribe(&session, &[range_node.clone(), units_node.clone()]).await;
    for _ in 0..2 {
        let (_, value) = next_change(&mut changes).await;
        assert!(value.status().is_good());
    }

    server
        .remove_tag(CONNECTION, GROUP, "Temperature")
        .expect("削除する");
    for _ in 0..2 {
        let (_, value) = next_change(&mut changes).await;
        assert_eq!(value.status(), StatusCode::BadNodeIdUnknown);
    }

    // レンジだけで登録し直す。EURange は同じ NodeId で戻り、EngineeringUnits は無いまま。
    server
        .add_tag_reading_with_metadata(
            CONNECTION,
            GROUP,
            "Temperature",
            reading(DataType::Double, 850.0),
            &TagMetadata::new().with_eu_range(EuRange::new(0.0, 900.0)),
        )
        .expect("登録し直す");
    let (node, value) = next_change(&mut changes).await;
    assert_eq!(node, range_node, "戻るのは EURange の購読");
    assert!(value.status().is_good());
    let range: Range = extension(&value);
    assert_eq!((range.low, range.high), (0.0, 900.0));
    assert_quiet(&mut changes).await;
    assert_eq!(
        read(&session, &units_node, AttributeId::Value)
            .await
            .status(),
        StatusCode::BadNodeIdUnknown,
        "登録し直さなかったプロパティは無いまま"
    );

    server.shutdown().await.expect("止める");
}

/// async-opcua だけで作る「他社のサーバー」。Description を持たない変数 `Level` に、
/// EngineeringUnits と EURange のプロパティを付ける。
async fn foreign_server() -> (String, u16, tokio::task::JoinHandle<()>, tempfile::TempDir) {
    use opcua::server::{
        ANONYMOUS_USER_TOKEN_ID, ServerBuilder, ServerEndpoint,
        address_space::Variable,
        diagnostics::NamespaceMetadata,
        node_manager::memory::{SimpleNodeManager, simple_node_manager},
    };
    use opcua::types::{DataTypeId, ExtensionObject, ObjectId, QualifiedName};

    let pki = tempfile::tempdir().expect("一時 PKI を作る");
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("待受を作る");
    let port = listener.local_addr().expect("ポート").port();
    let endpoint = format!("opc.tcp://127.0.0.1:{port}/");
    let (server, handle) = ServerBuilder::new()
        .application_name("foreign")
        .application_uri("urn:banto:opcua:foreign-test")
        .create_sample_keypair(true)
        .pki_dir(pki.path())
        .host("127.0.0.1")
        .port(port)
        .trust_client_certs(true)
        .add_endpoint(
            "none",
            ServerEndpoint::new_none("/", &[ANONYMOUS_USER_TOKEN_ID.to_owned()]),
        )
        .discovery_urls(vec![endpoint.clone()])
        .with_node_manager(simple_node_manager(
            NamespaceMetadata {
                namespace_uri: "urn:foreign".to_owned(),
                ..Default::default()
            },
            "foreign",
        ))
        .build()
        .expect("他社のサーバーを作る");
    let ns = handle
        .get_namespace_index("urn:foreign")
        .expect("名前空間がある");
    let manager = handle
        .node_managers()
        .get_of_type::<SimpleNodeManager>()
        .expect("node manager がある");
    {
        let mut space = manager.address_space().write();
        let level = NodeId::new(ns, "Level");
        assert!(space.insert::<_, NodeId>(Variable::new(&level, "Level", "Level", 3.5_f64), None));
        space.insert_reference(
            &ObjectId::ObjectsFolder.into(),
            &level,
            ReferenceTypeId::Organizes,
        );
        for (name, data_type, value) in [
            (
                "EngineeringUnits",
                DataTypeId::EUInformation,
                Variant::from(ExtensionObject::from_message(EUInformation {
                    namespace_uri: UNECE_UNITS_NAMESPACE_URI.into(),
                    unit_id: 5_067_858, // MTR
                    display_name: "m".into(),
                    description: LocalizedText::null(),
                })),
            ),
            (
                "EURange",
                DataTypeId::Range,
                Variant::from(ExtensionObject::from_message(Range {
                    low: 0.0,
                    high: 10.0,
                })),
            ),
        ] {
            let id = NodeId::new(ns, format!("Level.{name}"));
            assert!(space.insert::<_, NodeId>(
                Variable::new_data_value(
                    &id,
                    QualifiedName::new(0, name),
                    name,
                    data_type,
                    None,
                    None,
                    value,
                ),
                None,
            ));
            space.insert_reference(&level, &id, ReferenceTypeId::HasProperty);
        }
    }
    let task = tokio::spawn(async move {
        let _ = server.run_with(listener).await;
    });
    (endpoint, ns, task, pki)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn units_are_read_from_nodes_without_a_description_attribute() {
    use banto_opcua_client::{ClientConfig, OpcUaClient, model::StatusCode as ClientStatus};

    let (endpoint, ns, task, _pki) = foreign_server().await;
    let (session, _client_pki) = connect_to(&endpoint).await;
    let level = NodeId::new(ns, "Level");
    assert_eq!(
        read(&session, &level, AttributeId::Description)
            .await
            .status(),
        StatusCode::BadAttributeIdInvalid,
        "相手は Description を持たない"
    );

    let client = OpcUaClient::connect_with(ClientConfig::new(&endpoint))
        .await
        .expect("クライアントが接続する");
    let results = client
        .read_metadata(&["nsu=urn:foreign;s=Level".parse().expect("解析できる")])
        .await
        .expect("読む");
    assert_eq!(results[0].status, ClientStatus::GOOD);
    assert_eq!(
        results[0].metadata,
        TagMetadata::new()
            .with_engineering_units(EngineeringUnits::display("m").with_unece_code("MTR"))
            .with_eu_range(EuRange::new(0.0, 10.0)),
        "説明は無くても、単位とレンジは読む"
    );
    client.disconnect().await.expect("切断する");
    task.abort();
}
