//! 外部 OPC UA サーバー（opc-plc）を相手に、Hub のドライバ向けの部品を確かめる（ADR-0005 / ADR-0006）。
//!
//! まとめて Read（型ごと、`ns=` と `nsu=`、存在しないノード、解決できない URI）、
//! まとめて Write（書けるノード、書けないノード、型違い）と Write の後の Read、
//! 購読モードの最新値を順に確かめる。opc-plc のノードは 2.9 系のソースに従う。

use std::time::Duration;

use banto_opcua_client::{
    ClientConfig, ConnectedClient, OpcUaClient, SubscriptionSettings,
    model::{NodeAddress, ScalarValue, StatusCode},
};

/// opc-plc のノードの名前空間（`Namespaces.OpcPlcApplications`）。
const OPC_PLC: &str = "http://microsoft.com/Opc/OpcPlc/";

type Failure = Box<dyn std::error::Error>;

fn plc(name: &str) -> NodeAddress {
    format!("nsu={OPC_PLC};s={name}")
        .parse()
        .expect("固定の NodeId")
}

fn check(condition: bool, message: impl Into<String>) -> Result<(), Failure> {
    if condition {
        Ok(())
    } else {
        Err(message.into().into())
    }
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Failure> {
    let endpoint = std::env::args()
        .nth(1)
        .ok_or("接続先 URL を指定してください")?;
    let config = ClientConfig::new(endpoint).with_subscription(
        SubscriptionSettings::default()
            .with_publishing_interval(Duration::from_millis(200))
            .with_sampling_interval(Duration::from_millis(100)),
    );
    let client = OpcUaClient::connect_with(config).await?;

    let state = client.read_server_state().await?;
    check(
        state == 0,
        format!("ServerStatus/State が Running ではありません: {state}"),
    )?;

    read_types(&client).await?;
    write_then_read(&client).await?;
    subscribe(&client).await?;

    client.disconnect().await?;
    println!("OK");
    Ok(())
}

/// 型ごとの Read と、ノード単位の Bad。
async fn read_types(client: &ConnectedClient) -> Result<(), Failure> {
    let readings = client
        .read_batch(&[
            plc("StepUp"),
            plc("AlternatingBoolean"),
            plc("RandomSignedInt32"),
            plc("RandomUnsignedInt32"),
            "ns=3;s=FastUInt1".parse()?,
            plc("DoesNotExist"),
            "nsu=urn:banto:missing;i=1".parse()?,
        ])
        .await?;
    for reading in &readings {
        println!("READ {:?} {}", reading.value, reading.status);
    }
    check(readings.len() == 7, "結果の数が要求と合いません")?;
    check(
        matches!(readings[0].value, Some(ScalarValue::UInt32(_))),
        "StepUp が UInt32 ではありません",
    )?;
    check(
        matches!(readings[1].value, Some(ScalarValue::Boolean(_))),
        "AlternatingBoolean が Boolean ではありません",
    )?;
    check(
        matches!(readings[2].value, Some(ScalarValue::Int32(_))),
        "RandomSignedInt32 が Int32 ではありません",
    )?;
    check(
        matches!(readings[3].value, Some(ScalarValue::UInt32(_))),
        "RandomUnsignedInt32 が UInt32 ではありません",
    )?;
    check(
        matches!(readings[4].value, Some(ScalarValue::UInt32(_))),
        "ns=3;s=FastUInt1 が UInt32 ではありません（名前空間の番号が変わった?）",
    )?;
    check(
        readings[..5].iter().all(|reading| !reading.status.is_bad()),
        "既知のノードの StatusCode が Bad です",
    )?;
    for (position, reading) in readings.iter().enumerate().skip(5) {
        check(
            reading.value.is_none() && reading.status == StatusCode::BAD_NODE_ID_UNKNOWN,
            format!("{position} 番目が BadNodeIdUnknown ではありません"),
        )?;
    }
    Ok(())
}

/// 書けるノード・書けないノード・型違いと、Write の後の Read。
async fn write_then_read(client: &ConnectedClient) -> Result<(), Failure> {
    // fast ノードの更新を止め、書いた値が上書きされないようにする（opc-plc の設定ノード）。
    let stopped = client
        .write_batch(&[(plc("FastNumberOfUpdates"), ScalarValue::Int32(0))])
        .await?;
    check(
        stopped == [StatusCode::GOOD],
        format!("FastNumberOfUpdates を書けません: {stopped:?}"),
    )?;

    let statuses = client
        .write_batch(&[
            (plc("FastUInt1"), ScalarValue::UInt32(12_345)),
            (plc("AlternatingBoolean"), ScalarValue::Boolean(true)),
            (plc("FastUInt1"), ScalarValue::Double(1.0)),
            (plc("DoesNotExist"), ScalarValue::UInt32(1)),
        ])
        .await?;
    println!("WRITE {statuses:?}");
    check(statuses[0] == StatusCode::GOOD, "FastUInt1 を書けません")?;
    check(
        statuses[1] == StatusCode::BAD_NOT_WRITABLE,
        "AlternatingBoolean が BadNotWritable になりません",
    )?;
    check(
        statuses[2] == StatusCode::BAD_TYPE_MISMATCH,
        "型違いが BadTypeMismatch になりません",
    )?;
    check(
        statuses[3] == StatusCode::BAD_NODE_ID_UNKNOWN,
        "存在しないノードが BadNodeIdUnknown になりません",
    )?;

    let readings = client.read_batch(&[plc("FastUInt1")]).await?;
    check(
        readings[0].value == Some(ScalarValue::UInt32(12_345)),
        format!("書いた値を読めません: {:?}", readings[0]),
    )?;
    Ok(())
}

/// 購読モードで、変わり続けるノードの最新値が進むこと。
async fn subscribe(client: &ConnectedClient) -> Result<(), Failure> {
    let statuses = client
        .monitor(&[plc("StepUp"), plc("DoesNotExist")])
        .await?;
    println!("MONITOR {statuses:?}");
    check(statuses[0] == StatusCode::GOOD, "StepUp を監視できません")?;

    let mut first = None;
    let advanced = tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            let readings = client.latest().await?;
            if let Some(ScalarValue::UInt32(value)) = readings[0].value {
                match first {
                    None => first = Some(value),
                    Some(start) if value != start => {
                        println!("SUBSCRIBE StepUp {start} -> {value}");
                        check(
                            readings[1].value.is_none() && readings[1].status.is_bad(),
                            "存在しないノードが Bad になりません",
                        )?;
                        return Ok::<_, Failure>(());
                    }
                    Some(_) => {}
                }
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    })
    .await;
    advanced.map_err(|_| "購読の最新値が進みません")?
}
