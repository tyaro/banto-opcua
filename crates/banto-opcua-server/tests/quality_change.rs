//! フィルタ未指定の購読でも、値が同じまま品質だけ変わる通知を受け取る。

use std::time::Duration;

use banto_opcua_core::Quality;
use banto_opcua_server::{OpcUaServer, ServerConfig};
use opcua::{
    client::{ClientBuilder, DataChangeCallback, IdentityToken},
    crypto::SecurityPolicy,
    types::{
        AttributeId, DataValue, MessageSecurityMode, MonitoredItemCreateRequest, NodeId,
        ReadValueId, StatusCode, TimestampsToReturn, UserTokenPolicy, Variant,
    },
};
use tokio::sync::mpsc;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn quality_changes_are_reported_without_a_data_change_filter() {
    let server = OpcUaServer::start(ServerConfig::default().with_port(0))
        .await
        .expect("サーバーを起動する");
    let pki = tempfile::tempdir().expect("一時 PKI を作る");
    let mut client = ClientBuilder::new()
        .application_name("banto-opcua-quality-test")
        .application_uri("urn:banto:opcua:quality-test")
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
    let speed = NodeId::new(server.fixed_namespace_index(), "PLC1.Line1.Speed");
    // MonitoredItemCreateRequest の既定値がフィルタ未指定であることも固定する。
    let request: MonitoredItemCreateRequest = speed.clone().into();
    assert!(request.requested_parameters.filter.is_null());
    let items = session
        .create_monitored_items(subscription, TimestampsToReturn::Both, vec![request])
        .await
        .expect("監視項目を作る");
    assert_eq!(items.len(), 1);

    assert_change(&mut receiver, Some(Variant::Double(0.0)), StatusCode::Good).await;

    server
        .set_fixed_speed_reading(Some(42.5), Quality::Good)
        .expect("Good の値を更新する");
    assert_change(&mut receiver, Some(Variant::Double(42.5)), StatusCode::Good).await;

    server
        .set_fixed_speed_reading(Some(42.5), Quality::Stale)
        .expect("値を変えず Stale にする");
    assert_change(
        &mut receiver,
        Some(Variant::Double(42.5)),
        StatusCode::UncertainLastUsableValue,
    )
    .await;

    server
        .set_fixed_speed_reading(Some(42.5), Quality::Good)
        .expect("値を変えず Good に戻す");
    assert_change(&mut receiver, Some(Variant::Double(42.5)), StatusCode::Good).await;

    server
        .set_fixed_speed_reading(Some(42.5), Quality::Bad)
        .expect("Bad にして値を消す");
    assert_change(&mut receiver, Some(Variant::Empty), StatusCode::Bad).await;
    let read = session
        .read(
            &[ReadValueId {
                node_id: speed,
                attribute_id: AttributeId::Value as u32,
                ..Default::default()
            }],
            TimestampsToReturn::Both,
            0.0,
        )
        .await
        .expect("Bad の Read に成功する");
    assert_eq!(read[0].status(), StatusCode::Bad);
    assert_eq!(read[0].value, Some(Variant::Empty));

    server
        .set_fixed_speed_reading(Some(42.5), Quality::Good)
        .expect("Bad から Good に戻す");
    assert_change(&mut receiver, Some(Variant::Double(42.5)), StatusCode::Good).await;

    session.disconnect().await.expect("クライアントを切断する");
    task.abort();
    server.shutdown().await.expect("サーバーを停止する");
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
    assert!(change.source_timestamp.is_some());
}
