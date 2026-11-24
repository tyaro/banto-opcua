//! タグの説明・単位・レンジ・二値の文言（ADR-0008、#55）を OPC UA の実接続で確かめる。

use std::time::{Duration, SystemTime};

use banto_opcua_server::{
    OpcUaServer, RunningServer, ServerConfig, TagReading,
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
                server.endpoint(),
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
