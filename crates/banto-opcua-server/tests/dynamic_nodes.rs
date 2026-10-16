//! ノード削除後も購読を保ち、同じ NodeId の復帰とリネームを確認する。

use std::time::{Duration, SystemTime};

use banto_opcua_server::{
    OpcUaServer, ServerConfig, TagReading,
    model::{DataType, MappedValue, Quality, SampleTimestamps},
};
use opcua::{
    client::{ClientBuilder, DataChangeCallback, IdentityToken, Session},
    crypto::SecurityPolicy,
    types::{
        AttributeId, BrowseDescription, BrowseDirection, DataValue, MessageSecurityMode,
        MonitoredItemCreateRequest, NodeId, NumericRange, ObjectId, ReadValueId, ReferenceTypeId,
        StatusCode, TimestampsToReturn, UserTokenPolicy, Variant,
    },
};
use tokio::sync::mpsc;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deleted_node_notifies_and_reappearing_node_resumes_the_same_subscription() {
    let server = OpcUaServer::start(ServerConfig::default().with_port(0))
        .await
        .expect("サーバーを起動する");
    server
        .add_tag("Banto", "Cell1", "Speed", MappedValue::Double(10.0))
        .expect("ルートと同名の接続も独立したフォルダーで登録する");
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
    let same_named_connection = browse_child(&session, NodeId::new(ns, "Banto"), "Banto").await;
    assert_ne!(same_named_connection, NodeId::new(ns, "Banto"));
    assert!(
        browse_names(&session, same_named_connection)
            .await
            .contains(&"Cell1".to_owned())
    );
    assert_eq!(
        read_value(&session, &NodeId::new(ns, "Banto.Cell1.Speed"))
            .await
            .value,
        Some(Variant::Double(10.0))
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

    // 範囲指定を持つ MonitoredItem に、再登録時も通常のサンプリング結果を送る。
    let (ranged_sender, mut ranged_receiver) = mpsc::unbounded_channel();
    let ranged_subscription = session
        .create_subscription(
            Duration::from_millis(100),
            30,
            10,
            0,
            0,
            true,
            DataChangeCallback::new(move |value, _item| {
                let _ = ranged_sender.send(value);
            }),
        )
        .await
        .expect("範囲指定の購読を作る");
    let mut ranged_request: MonitoredItemCreateRequest = speed.clone().into();
    ranged_request.item_to_monitor.index_range = NumericRange::Index(0);
    let ranged_items = session
        .create_monitored_items(
            ranged_subscription,
            TimestampsToReturn::Both,
            vec![ranged_request],
        )
        .await
        .expect("範囲指定の監視項目を作る");
    assert_eq!(ranged_items.len(), 1);
    assert_change(
        &mut ranged_receiver,
        None,
        StatusCode::BadIndexRangeDataMismatch,
    )
    .await;

    server
        .remove_tag("PLC2", "Cell1", "Speed")
        .expect("タグを削除する");
    assert_change(&mut receiver, None, StatusCode::BadNodeIdUnknown).await;
    assert_change(&mut ranged_receiver, None, StatusCode::BadNodeIdUnknown).await;
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
    assert_change(
        &mut ranged_receiver,
        None,
        StatusCode::BadIndexRangeDataMismatch,
    )
    .await;
    assert_eq!(
        read_value(&session, &speed).await.value,
        Some(Variant::Double(11.0))
    );

    // リネームは旧ノードの削除と新ノードの追加で表す。
    server
        .remove_tag("PLC2", "Cell1", "Speed")
        .expect("旧名を削除する");
    assert_change(&mut receiver, None, StatusCode::BadNodeIdUnknown).await;
    assert_change(&mut ranged_receiver, None, StatusCode::BadNodeIdUnknown).await;
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

    // Hub のタグ名はドットを許す。フォルダー NodeId の衝突を避けながら
    // BrowseName と外部名の Variable NodeId をそのまま公開する。
    server
        .add_tag("PLC.3", "Cell.2", "Speed.1", MappedValue::Double(13.0))
        .expect("ドット入り名のタグを登録する");
    let connection = browse_child(&session, NodeId::new(ns, "Banto"), "PLC.3").await;
    let group = browse_child(&session, connection, "Cell.2").await;
    let dotted = browse_child(&session, group, "Speed.1").await;
    assert_eq!(dotted, NodeId::new(ns, "PLC.3.Cell.2.Speed.1"));
    assert_eq!(
        read_value(&session, &dotted).await.value,
        Some(Variant::Double(13.0))
    );
    server
        .remove_tag("PLC.3", "Cell.2", "Speed.1")
        .expect("ドット入り名のタグを削除する");
    assert_eq!(
        read_value(&session, &dotted).await.status(),
        StatusCode::BadNodeIdUnknown
    );

    // 同じ外部名になる別の分割から、既存タグを更新・削除してはならない。
    server
        .add_tag("a.b", "c", "d", MappedValue::Double(14.0))
        .expect("元の階層に登録する");
    let ambiguous = NodeId::new(ns, "a.b.c.d");
    for (connection, group, tag) in [("a", "b.c", "d"), ("a", "b", "c.d")] {
        assert!(
            server
                .add_tag(connection, group, tag, MappedValue::Double(15.0))
                .is_err(),
            "別の分割での二重登録を拒否する"
        );
        assert!(
            server
                .update_tag_reading(
                    connection,
                    group,
                    tag,
                    TagReading::new(
                        DataType::Double,
                        Some(15.0),
                        Quality::Good,
                        SampleTimestamps::new(None, SystemTime::now()),
                    ),
                )
                .is_err(),
            "別の分割からの更新を拒否する"
        );
        assert!(
            server.remove_tag(connection, group, tag).is_err(),
            "別の分割からの削除を拒否する"
        );
        assert_eq!(
            read_value(&session, &ambiguous).await.value,
            Some(Variant::Double(14.0)),
            "拒否後も元の値を保持する"
        );
    }
    server
        .remove_tag("a.b", "c", "d")
        .expect("元の分割からは削除できる");

    session.disconnect().await.expect("クライアントを切断する");
    task.abort();
    server.shutdown().await.expect("サーバーを停止する");
}

async fn browse_names(session: &Session, parent: NodeId) -> Vec<String> {
    browse_children(session, parent)
        .await
        .into_iter()
        .map(|(name, _)| name)
        .collect()
}

async fn browse_child(session: &Session, parent: NodeId, name: &str) -> NodeId {
    browse_children(session, parent)
        .await
        .into_iter()
        .find(|(child_name, _)| child_name == name)
        .unwrap_or_else(|| panic!("BrowseName {name} が見つからない"))
        .1
}

async fn browse_children(session: &Session, parent: NodeId) -> Vec<(String, NodeId)> {
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
        .map(|reference| {
            (
                reference.browse_name.name.to_string(),
                reference.node_id.node_id.clone(),
            )
        })
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
