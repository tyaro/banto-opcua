//! 外部クライアント試験用に、固定タグと書き込み可のタグを空きポートで公開する。
//!
//! 標準入力のコマンド: `update` は固定タグを更新し、`reflect` は最後に受けた
//! 書き込みを機器から戻った値として `update_tag_reading` で反映する。
//! 書き込みハンドラーは受けた書き込みを `WRITE` 行で標準出力へ書く。

use std::{
    io::{self, Write},
    sync::{Arc, Mutex},
    time::SystemTime,
};

use banto_opcua_server::{
    OpcUaServer, ServerConfig, TagReading, WriteFuture, WriteHandler,
    model::{
        DataType, MappedValue, Quality, SampleTimestamps, WritePrincipal, WriteRejection,
        WriteRequest,
    },
};

const CONNECTION: &str = "PLC2";
const GROUP: &str = "Cell1";

/// 受けた書き込みを記録する。`Locked` は書き込みを受け付けていないタグとして拒否する。
struct Recorder {
    last: Arc<Mutex<Option<WriteRequest>>>,
}

impl WriteHandler for Recorder {
    fn write(&self, request: WriteRequest) -> WriteFuture {
        let last = self.last.clone();
        Box::pin(async move {
            if request.tag == "Locked" {
                return Err(WriteRejection::OutOfService);
            }
            let value = match request.value {
                MappedValue::Boolean(bit) => bit.to_string(),
                MappedValue::Double(number) => number.to_string(),
            };
            let principal = match request.principal {
                WritePrincipal::Anonymous => "anonymous",
                _ => "other",
            };
            println!(
                "WRITE {}.{}.{} {value} {principal} {}",
                request.connection, request.group, request.tag, request.client.application_uri
            );
            io::stdout().flush().map_err(|_| WriteRejection::Internal)?;
            *last.lock().map_err(|_| WriteRejection::Internal)? = Some(request);
            Ok(())
        })
    }
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let last = Arc::new(Mutex::new(None));
    let server = OpcUaServer::start_with_write_handler(
        ServerConfig::default().with_port(0),
        Arc::new(Recorder { last: last.clone() }),
    )
    .await?;
    for (tag, value) in [
        ("Setpoint", MappedValue::Double(0.0)),
        ("Enable", MappedValue::Boolean(false)),
        ("Locked", MappedValue::Double(0.0)),
    ] {
        server.add_tag(CONNECTION, GROUP, tag, value)?;
        server.set_tag_writable(CONNECTION, GROUP, tag, true)?;
    }
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
            }
            "reflect" => {
                let request = last
                    .lock()
                    .map_err(|_| "書き込みの記録を読めません")?
                    .take()
                    .ok_or("反映する書き込みがありません")?;
                let (data_type, value) = match request.value {
                    MappedValue::Boolean(bit) => (DataType::Boolean, f64::from(u8::from(bit))),
                    MappedValue::Double(number) => (DataType::Double, number),
                };
                let now = SystemTime::now();
                server.update_tag_reading(
                    &request.connection,
                    &request.group,
                    &request.tag,
                    TagReading::new(
                        data_type,
                        Some(value),
                        Quality::Good,
                        SampleTimestamps::new(Some(now), now),
                    ),
                )?;
                println!("REFLECTED");
            }
            "quit" | "" => break,
            command => return Err(format!("不明なコマンド: {command}").into()),
        }
        io::stdout().flush()?;
    }

    server.shutdown().await?;
    Ok(())
}
