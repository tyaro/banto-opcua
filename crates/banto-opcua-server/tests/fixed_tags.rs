//! Phase 1 の固定タグを実際の Browse / Read / Subscription サービスで確認する。

use std::{sync::mpsc, time::Duration};

use banto_opcua_server::{OpcUaServer, ServerConfig};
use opcua::{
    client::{ClientBuilder, DataChangeCallback, IdentityToken, Session},
    crypto::SecurityPolicy,
    types::{
        AttributeId, BrowseDescription, BrowseDirection, DataTypeId, DataValue,
        MessageSecurityMode, MonitoredItemCreateRequest, NodeId, ObjectId, ReadValueId,
        ReferenceTypeId, StatusCode, TimestampsToReturn, UserTokenPolicy, VariableId, Variant,
        WriteValue,
    },
};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn fixed_tags_can_be_browsed_read_and_subscribed() {
    let config = ServerConfig::default()
        .with_port(0)
        .with_namespace_uri("urn:banto:opcua:phase1-test-tags");
    let namespace_uri = config.namespace_uri.clone();
    let server = OpcUaServer::start(config)
        .await
        .expect("サーバーを起動する");

    let pki = tempfile::tempdir().expect("一時 PKI を作る");
    let mut client = ClientBuilder::new()
        .application_name("banto-opcua-phase1-test")
        .application_uri("urn:banto:opcua:phase1-test")
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

    let banto = browse_child(&session, ObjectId::ObjectsFolder.into(), "Banto").await;
    let ns = banto.namespace;
    assert_ne!(ns, 0);
    let namespaces = read_attribute(
        &session,
        &VariableId::Server_NamespaceArray.into(),
        AttributeId::Value,
    )
    .await;
    let Variant::Array(namespaces) = namespaces else {
        panic!("NamespaceArray は文字列配列である");
    };
    assert_eq!(
        namespaces.values[ns as usize],
        Variant::String(namespace_uri.into())
    );
    assert_eq!(banto, NodeId::new(ns, "Banto"));
    let plc = browse_child(&session, banto, "PLC1").await;
    let line = browse_child(&session, plc, "Line1").await;
    let speed = browse_child(&session, line.clone(), "Speed").await;
    let running = browse_child(&session, line, "Running").await;
    assert_eq!(speed, NodeId::new(ns, "PLC1.Line1.Speed"));
    assert_eq!(running, NodeId::new(ns, "PLC1.Line1.Running"));

    assert_eq!(
        read_attribute(&session, &speed, AttributeId::Value).await,
        Variant::Double(0.0)
    );
    assert_eq!(
        read_attribute(&session, &running, AttributeId::Value).await,
        Variant::Boolean(false)
    );
    assert_eq!(
        read_attribute(&session, &speed, AttributeId::DataType).await,
        Variant::NodeId(Box::new(DataTypeId::Double.into()))
    );
    assert_eq!(
        read_attribute(&session, &running, AttributeId::DataType).await,
        Variant::NodeId(Box::new(DataTypeId::Boolean.into()))
    );
    for id in [&speed, &running] {
        for attribute in [AttributeId::AccessLevel, AttributeId::UserAccessLevel] {
            assert_eq!(
                read_attribute(&session, id, attribute).await,
                Variant::Byte(1)
            );
        }
    }
    let write_results = session
        .write(&[WriteValue::value_attr(speed.clone(), Variant::Double(99.0))])
        .await
        .expect("Write のサービス応答がある");
    assert!(
        write_results[0].is_bad(),
        "読み取り専用タグの Write を拒否する"
    );
    assert_eq!(
        read_attribute(&session, &speed, AttributeId::Value).await,
        Variant::Double(0.0)
    );

    let (sender, receiver) = mpsc::channel();
    let subscription = session
        .create_subscription(
            Duration::from_millis(100),
            30,
            10,
            0,
            0,
            true,
            DataChangeCallback::new(move |value, _item| {
                if matches!(
                    value.value.as_ref(),
                    Some(Variant::Double(_) | Variant::Boolean(_))
                ) {
                    let _ = sender.send(value);
                }
            }),
        )
        .await
        .expect("購読を作る");
    let mut requests: Vec<MonitoredItemCreateRequest> =
        vec![speed.clone().into(), running.clone().into()];
    for request in &mut requests {
        request.requested_parameters.queue_size = 10;
    }
    let items = session
        .create_monitored_items(subscription, TimestampsToReturn::Both, requests)
        .await
        .expect("監視項目を作る");
    assert_eq!(items.len(), 2);
    assert!(server.set_fixed_speed(f64::NAN).is_err());
    server.set_fixed_speed(42.5).expect("固定値を更新する");
    server
        .set_fixed_running(true)
        .expect("Boolean 値を更新する");
    tokio::task::spawn_blocking(move || {
        let mut got_speed = false;
        let mut got_running = false;
        while !got_speed || !got_running {
            let value = receiver
                .recv_timeout(Duration::from_secs(5))
                .expect("DataChange が届く");
            match value.value {
                Some(Variant::Double(42.5)) => {
                    assert_eq!(value.status(), StatusCode::Good);
                    assert!(value.source_timestamp.is_some());
                    got_speed = true;
                }
                Some(Variant::Boolean(true)) => {
                    assert_eq!(value.status(), StatusCode::Good);
                    assert!(value.source_timestamp.is_some());
                    got_running = true;
                }
                _ => {}
            }
        }
    })
    .await
    .expect("受信タスクが終了する");
    assert_eq!(
        read_attribute(
            &session,
            &NodeId::new(ns, "PLC1.Line1.Speed"),
            AttributeId::Value
        )
        .await,
        Variant::Double(42.5)
    );
    assert_eq!(
        read_attribute(&session, &running, AttributeId::Value).await,
        Variant::Boolean(true)
    );
    let updated = read_data_value(&session, &running, AttributeId::Value).await;
    assert_eq!(updated.status(), StatusCode::Good);
    assert!(updated.source_timestamp.is_some());
    assert!(updated.server_timestamp.is_some());

    session.disconnect().await.expect("クライアントを切断する");
    task.abort();
    server.shutdown().await.expect("サーバーを停止する");
}

async fn browse_child(session: &Session, parent: NodeId, name: &str) -> NodeId {
    let results = session
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
    let result = results.into_iter().next().expect("Browse 結果がある");
    assert_eq!(result.status_code, StatusCode::Good);
    result
        .references
        .unwrap_or_default()
        .into_iter()
        .find(|reference| reference.browse_name.name.as_ref() == name)
        .unwrap_or_else(|| panic!("{name} が Browse できない"))
        .node_id
        .node_id
}

async fn read_attribute(session: &Session, id: &NodeId, attribute: AttributeId) -> Variant {
    read_data_value(session, id, attribute)
        .await
        .value
        .expect("値がある")
}

async fn read_data_value(session: &Session, id: &NodeId, attribute: AttributeId) -> DataValue {
    let values = session
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
        .expect("Read に成功する");
    let value = values.into_iter().next().expect("Read 値がある");
    assert_eq!(value.status(), StatusCode::Good);
    value
}
