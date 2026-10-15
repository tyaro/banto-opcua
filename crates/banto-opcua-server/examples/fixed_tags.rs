//! UaExpert で固定タグの Browse / Read / Subscribe を試すサーバー。

use std::io;

use banto_opcua_server::{OpcUaServer, ServerConfig};

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let server = OpcUaServer::start(ServerConfig::default()).await?;
    println!("接続先: {}", server.endpoint());
    println!(
        "固定タグの名前空間インデックス: {}",
        server.fixed_namespace_index()
    );
    println!("Enter: Speed と Running を更新 / q: 停止");

    let mut speed = 0.0;
    let mut running = false;
    loop {
        let (bytes, line) = tokio::task::spawn_blocking(|| {
            let mut line = String::new();
            let bytes = io::stdin().read_line(&mut line)?;
            Ok::<_, io::Error>((bytes, line))
        })
        .await??;
        if bytes == 0 || line.trim() == "q" {
            break;
        }
        speed += 1.0;
        running = !running;
        server.set_fixed_speed(speed)?;
        server.set_fixed_running(running)?;
        println!("Speed={speed}, Running={running}");
    }

    server.shutdown().await?;
    Ok(())
}
