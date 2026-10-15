//! 外部クライアント試験用に固定タグを空きポートで公開する。

use std::io::{self, Write};

use banto_opcua_server::{OpcUaServer, ServerConfig};

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let server = OpcUaServer::start(ServerConfig::default().with_port(0)).await?;
    println!("ENDPOINT={}", server.endpoint());
    io::stdout().flush()?;

    loop {
        let line = tokio::task::spawn_blocking(|| {
            let mut line = String::new();
            io::stdin().read_line(&mut line)?;
            Ok::<_, io::Error>(line)
        })
        .await??;
        match line.trim() {
            "update" => {
                server.set_fixed_speed(42.5)?;
                server.set_fixed_running(true)?;
                println!("UPDATED");
                io::stdout().flush()?;
            }
            "quit" | "" => break,
            command => return Err(format!("不明なコマンド: {command}").into()),
        }
    }

    server.shutdown().await?;
    Ok(())
}
