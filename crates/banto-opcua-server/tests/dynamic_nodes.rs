//! ノード削除後も購読を保ち、同じ NodeId の復帰とリネームを確認する。

use std::time::Duration;

use banto_opcua_server::{OpcUaServer, ServerConfig, model::MappedValue};
use opcua::{
    client::{ClientBuilder, DataChangeCallback, IdentityToken, Session},
    crypto::SecurityPolicy,
    types::{
        AttributeId, BrowseDescription, BrowseDirection, DataValue, MessageSecurityMode,
        MonitoredItemCreateRequest, NodeId, ObjectId, ReadValueId, ReferenceTypeId, StatusCode,
        TimestampsToReturn, UserTokenPolicy, Variant,
    },
};
use tokio::sync::mpsc;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deleted_node_notifies_and_reappearing_node_resumes_the_same_subscription() {
    let server = OpcUaServer::start(ServerConfig::default().with_port(0))
        .await
        .expect("サーバーを起動する");
    server
        .add_tag("PLC2", "Cell1", "Speed", MappedValue::Double(10.0))
        .expect("動的タグを登録する");
    let pki = tempfile::tempdir().expect("一時 PKI を作る");
    let mut client = ClientBuilder::new()
        .application_name("banto-opcua-dynamic-test")
        .application_uri("urn:banto:opcua:dynamic-test")
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
    let task = event_loop.spawn();
    assert!(
        tokio::time::timeout(Duration::from_secs(10), session.wait_for_connection())
            .await
            .expect("接続待ちが時間切れ")
    );

    let ns = server.namespace_index();
    let banto = NodeId::new(ns, "Banto");
    assert!(
        browse_names(&session, ObjectId::ObjectsFolder.into())
            .await
            .contains(&"Banto".to_owned())
    );
    assert!(
        browse_names(&session, banto)
            .await
            .contains(&"PLC2".to_owned())
    );
    let cell = NodeId::new(ns, "PLC2.Cell1");
    assert!(
        browse_names(&session, NodeId::new(ns, "PLC2"))
            .await
            .contains(&"Cell1".to_owned())
    );
    assert!(
        browse_names(&session, cell.clone())
            .await
            .contains(&"Speed".to_owned())
    );

    let speed = NodeId::new(ns, "PLC2.Cell1.Speed");
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
    let request: MonitoredItemCreateRequest = speed.clone().into();
    let items = session
        .create_monitored_items(subscription, TimestampsToReturn::Both, vec![request])
        .await
        .expect("監視項目を作る");
    assert_eq!(items.len(), 1);
    assert_change(&mut receiver, Some(Variant::Double(10.0)), StatusCode::Good).await;

    server
        .remove_tag("PLC2", "Cell1", "Speed")
        .expect("タグを削除する");
    assert_change(
        &mut receiver,
        Some(Variant::Empty),
        StatusCode::BadNodeIdUnknown,
    )
    .await;
    assert_eq!(
        read_value(&session, &speed).await.status(),
        StatusCode::BadNodeIdUnknown
    );
    assert!(
        !browse_names(&session, cell.clone())
            .await
            .contains(&"Speed".to_owned())
    );

    server
        .add_tag("PLC2", "Cell1", "Speed", MappedValue::Double(11.0))
        .expect("同じ NodeId を再登録する");
    assert_change(&mut receiver, Some(Variant::Double(11.0)), StatusCode::Good).await;
    assert_eq!(
        read_value(&session, &speed).await.value,
        Some(Variant::Double(11.0))
    );

    // リネームは旧ノードの削除と新ノードの追加で表す。
    server
        .remove_tag("PLC2", "Cell1", "Speed")
        .expect("旧名を削除する");
    assert_change(
        &mut receiver,
        Some(Variant::Empty),
        StatusCode::BadNodeIdUnknown,
    )
    .await;
    server
        .add_tag("PLC2", "Cell1", "Velocity", MappedValue::Double(12.0))
        .expect("新名を登録する");
    let names = browse_names(&session, cell).await;
    assert!(!names.contains(&"Speed".to_owned()));
    assert!(names.contains(&"Velocity".to_owned()));
    assert_eq!(
        read_value(&session, &speed).await.status(),
        StatusCode::BadNodeIdUnknown
    );
    let velocity = NodeId::new(ns, "PLC2.Cell1.Velocity");
    assert_eq!(
        read_value(&session, &velocity).await.value,
        Some(Variant::Double(12.0))
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(300), receiver.recv())
            .await
            .is_err(),
        "旧 NodeId の MonitoredItem に新ノードの値は届かない"
    );

    session.disconnect().await.expect("クライアントを切断する");
    task.abort();
    server.shutdown().await.expect("サーバーを停止する");
}

async fn browse_names(session: &Session, parent: NodeId) -> Vec<String> {
    let result = session
        .browse(
            &[BrowseDescription {
                node_id: parent,
                browse_direction: BrowseDirection::Forward,
                reference_type_id: ReferenceTypeId::HierarchicalReferences.into(),
                include_subtypes: true,
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
        .as_ref()
        .into_iter()
        .flatten()
        .map(|reference| reference.browse_name.name.to_string())
        .collect()
}

async fn read_value(session: &Session, id: &NodeId) -> DataValue {
    session
        .read(
            &[ReadValueId {
                node_id: id.clone(),
                attribute_id: AttributeId::Value as u32,
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

async fn assert_change(
    receiver: &mut mpsc::UnboundedReceiver<DataValue>,
    expected_value: Option<Variant>,
    expected_status: StatusCode,
) {
    let change = tokio::time::timeout(Duration::from_secs(5), receiver.recv())
        .await
        .expect("DataChange の待ち時間が切れた")
        .expect("DataChange が届く");
    assert_eq!(change.value, expected_value);
    assert_eq!(change.status(), expected_status);
}
