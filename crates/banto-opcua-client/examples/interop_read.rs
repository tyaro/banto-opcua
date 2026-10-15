//! 外部 OPC UA サーバーの Read を確認する。

use banto_opcua_client::{OpcUaClient, model::NodeId};

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let endpoint = std::env::args()
        .nth(1)
        .ok_or("接続先 URL を指定してください")?;
    let client = OpcUaClient::connect(&endpoint).await?;
    let state = client.read_server_state().await?;
    if state != 0 {
        return Err(format!("ServerStatus/State が Running ではありません: {state}").into());
    }
    let value = client.read_u32(&NodeId::string(3, "FastUInt1")).await?;
    println!("FastUInt1={value}");
    client.disconnect().await?;
    Ok(())
}
