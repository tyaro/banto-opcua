//! 外部クライアント試験用に、固定タグと書き込み可のタグを空きポートで公開する。
//!
//! 標準入力のコマンド: `update` は固定タグを更新し、`reflect` は最後に受けた
//! 書き込みを機器から戻った値として `update_tag_reading` で反映する。
//! 書き込みハンドラーは受けた書き込みを `WRITE` 行で標準出力へ書く。
//!
//! エンドポイントは `None`（Anonymous だけ）と、暗号化したもの（Anonymous と UserName）を
//! 開く。利用者は `operator` / `secret` だけで、知らないクライアントの証明書も信頼する
//! （試験用、ADR-0009）。

use std::{
    io::{self, Write},
    sync::{Arc, Mutex},
    time::SystemTime,
};

use banto_opcua_server::{
    AuthenticateFuture, OpcUaServer, ServerConfig, ServerHandlers, TagReading, UserAuthenticator,
    WriteFuture, WriteHandler,
    model::{
        MappedValue, Quality, SampleTimestamps, UserNameLogin, UserRejection, WritePrincipal,
        WriteRejection, WriteRequest,
    },
};

const CONNECTION: &str = "PLC2";
const GROUP: &str = "Cell1";

/// `operator` / `secret` だけを通す。
struct Users;

impl UserAuthenticator for Users {
    fn authenticate(&self, login: UserNameLogin) -> AuthenticateFuture {
        Box::pin(async move {
            match (login.user_name.as_str(), login.password.expose()) {
                ("operator", "secret") => Ok(()),
                ("operator", _) => Err(UserRejection::WrongPassword),
                _ => Err(UserRejection::UnknownUser),
            }
        })
    }
}

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
            let value = request.value.to_string();
            let principal = match &request.principal {
                WritePrincipal::Anonymous => "anonymous".to_owned(),
                WritePrincipal::User(name) => format!("user:{name}"),
                _ => "other".to_owned(),
            };
            let line = format!(
                "WRITE {}.{}.{} {value} {principal} {}",
                request.connection, request.group, request.tag, request.client.application_uri
            );
            // 記録してから出力する。WRITE を読んだ直後の reflect でも記録が済んでいる。
            *last.lock().map_err(|_| WriteRejection::Internal)? = Some(request);
            println!("{line}");
            io::stdout().flush().map_err(|_| WriteRejection::Internal)?;
            Ok(())
        })
    }
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let last = Arc::new(Mutex::new(None));
    let server = OpcUaServer::start_with_handlers(
        ServerConfig::default()
            .with_none_endpoint(true)
            .with_trust_all_client_certificates(true)
            .with_port(0),
        ServerHandlers::default()
            .with_write_handler(Arc::new(Recorder { last: last.clone() }))
            .with_user_authenticator(Arc::new(Users)),
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
                let now = SystemTime::now();
                server.update_tag_reading(
                    &request.connection,
                    &request.group,
                    &request.tag,
                    TagReading::typed(
                        request.value.data_type(),
                        Some(request.value),
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
