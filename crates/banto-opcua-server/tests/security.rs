//! 暗号化したエンドポイント、UserName の照合、証明書の信頼の一覧（ADR-0009）を、
//! 本リポジトリのサーバーとクライアントの組で確かめる。

use std::{path::Path, sync::Arc};

use banto_opcua_client::{
    ClientConfig, Error as ClientError, OpcUaClient, TrustList, TrustState,
    model::{MessageSecurityMode, NodeAddress, ScalarValue, SecurityPolicy, UserTokenType},
};
use banto_opcua_server::{
    AuthenticateFuture, OpcUaServer, RunningServer, SecurityEventKind, ServerConfig,
    ServerHandlers, UserAuthenticator, WriteFuture, WriteHandler,
    model::{MappedValue, UserNameLogin, UserRejection, WritePrincipal, WriteRequest},
};
use tokio::sync::mpsc;

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

/// 書き込みの主体を記録する。
struct Principals(mpsc::UnboundedSender<WritePrincipal>);

impl WriteHandler for Principals {
    fn write(&self, request: WriteRequest) -> WriteFuture {
        self.0.send(request.principal).expect("主体を記録する");
        Box::pin(async { Ok(()) })
    }
}

fn secure(endpoint: &str, pki: &Path) -> ClientConfig {
    ClientConfig::new(endpoint)
        .with_security(
            SecurityPolicy::Basic256Sha256,
            MessageSecurityMode::SignAndEncrypt,
        )
        .with_pki_dir(pki)
        .with_application("banto-test-client", "urn:banto:test:client")
}

async fn start_with_users(config: ServerConfig) -> RunningServer {
    OpcUaServer::start_with_handlers(
        config.with_port(0),
        ServerHandlers::default().with_user_authenticator(Arc::new(Users)),
    )
    .await
    .expect("サーバーを起動する")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn default_server_advertises_only_encrypted_endpoints() {
    let server = start_with_users(ServerConfig::default()).await;

    let endpoints = OpcUaClient::endpoints(&ClientConfig::new(server.endpoint()))
        .await
        .expect("エンドポイントを読む");
    assert_eq!(endpoints.len(), 6, "{endpoints:?}");
    for endpoint in &endpoints {
        assert!(
            matches!(
                endpoint.security_policy,
                Some(
                    SecurityPolicy::Basic256Sha256
                        | SecurityPolicy::Aes128Sha256RsaOaep
                        | SecurityPolicy::Aes256Sha256RsaPss
                )
            ),
            "{endpoint:?}"
        );
        assert_ne!(endpoint.security_mode, Some(MessageSecurityMode::None));
        assert_eq!(
            endpoint.user_token_types,
            [UserTokenType::Anonymous, UserTokenType::UserName]
        );
        assert_eq!(
            endpoint
                .server_certificate
                .as_ref()
                .expect("証明書がある")
                .thumbprint,
            server.certificate().thumbprint
        );
    }

    let err = OpcUaClient::connect(server.endpoint())
        .await
        .expect_err("None のエンドポイントは無い");
    assert!(
        matches!(
            err,
            ClientError::EndpointNotFound {
                security_policy: SecurityPolicy::None,
                ..
            }
        ),
        "{err}"
    );
    server.shutdown().await.expect("停止する");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn none_endpoint_accepts_anonymous_only() {
    let server = start_with_users(ServerConfig::default().with_none_endpoint(true)).await;
    let endpoints = OpcUaClient::endpoints(&ClientConfig::new(server.endpoint()))
        .await
        .expect("エンドポイントを読む");
    let none: Vec<_> = endpoints
        .iter()
        .filter(|endpoint| endpoint.security_policy == Some(SecurityPolicy::None))
        .collect();
    assert_eq!(none.len(), 1);
    assert_eq!(none[0].user_token_types, [UserTokenType::Anonymous]);
    server.shutdown().await.expect("停止する");
}

/// 初めて会う相手の証明書は、どちらの側でも拒否の一覧に置かれ、信頼に移すと通る。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn certificates_are_trusted_explicitly_on_both_sides() {
    let root = tempfile::tempdir().expect("試験用ディレクトリ");
    let client_pki = root.path().join("client");
    let server = OpcUaServer::start(
        ServerConfig::default()
            .with_port(0)
            .with_pki_dir(root.path().join("server")),
    )
    .await
    .expect("サーバーを起動する");
    let config = secure(server.endpoint(), &client_pki);

    let err = OpcUaClient::connect_with(config.clone())
        .await
        .expect_err("サーバーの証明書をまだ信頼していない");
    let ClientError::ServerCertificateUntrusted { certificate } = err else {
        panic!("サーバーの証明書を信頼していないことを返す: {err}");
    };
    assert_eq!(certificate, *server.certificate());
    let client_list = TrustList::new(&client_pki);
    let listed = client_list.certificates().expect("一覧を読む");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].state, TrustState::Rejected);
    client_list
        .trust(&certificate.thumbprint)
        .expect("サーバーの証明書を信頼する");

    let err = OpcUaClient::connect_with(config.clone())
        .await
        .expect_err("サーバーはまだこちらの証明書を信頼していない");
    assert!(
        matches!(err, ClientError::ClientCertificateRejected(_)),
        "{err}"
    );
    let pending = server.trust_list().certificates().expect("一覧を読む");
    assert_eq!(pending.len(), 1, "初めて来た証明書は拒否の一覧に置かれる");
    assert_eq!(pending[0].state, TrustState::Rejected);
    assert!(pending[0].info.subject.contains("banto-test-client"));
    server
        .trust_list()
        .trust(&pending[0].info.thumbprint)
        .expect("クライアントの証明書を信頼する");

    let client = OpcUaClient::connect_with(config)
        .await
        .expect("両方が信頼すれば接続できる");
    assert_eq!(client.read_server_state().await.expect("状態を読む"), 0);
    client.disconnect().await.expect("切断する");

    server
        .trust_list()
        .reject(&pending[0].info.thumbprint)
        .expect("拒否へ戻す");
    let err = OpcUaClient::connect_with(secure(server.endpoint(), &client_pki))
        .await
        .expect_err("拒否に戻した証明書は断る");
    assert!(
        matches!(err, ClientError::ClientCertificateRejected(_)),
        "{err}"
    );
    server.shutdown().await.expect("停止する");
}

/// 利用者名は利用者の関数で照合し、断った理由はサーバーの記録にだけ残る。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn user_names_are_checked_by_the_host() {
    let root = tempfile::tempdir().expect("試験用ディレクトリ");
    let (principals, mut received) = mpsc::unbounded_channel();
    let server = OpcUaServer::start_with_handlers(
        ServerConfig::default()
            .with_port(0)
            .with_anonymous(false)
            .with_trust_all_client_certificates(true),
        ServerHandlers::default()
            .with_user_authenticator(Arc::new(Users))
            .with_write_handler(Arc::new(Principals(principals))),
    )
    .await
    .expect("サーバーを起動する");
    server
        .add_tag("PLC2", "Cell1", "Setpoint", MappedValue::Double(0.0))
        .expect("タグを足す");
    server
        .set_tag_writable("PLC2", "Cell1", "Setpoint", true)
        .expect("書き込み可にする");
    let config = secure(server.endpoint(), root.path()).with_trust_all_server_certificates(true);

    let err = OpcUaClient::connect_with(config.clone())
        .await
        .expect_err("Anonymous は広告されていない");
    assert!(
        matches!(
            err,
            ClientError::EndpointNotFound {
                token_type: UserTokenType::Anonymous,
                ..
            }
        ),
        "{err}"
    );
    for (user, password) in [("operator", "wrong"), ("guest", "secret")] {
        let err = OpcUaClient::connect_with(config.clone().with_user_name(user, password))
            .await
            .expect_err("照合を通らない");
        assert!(matches!(err, ClientError::UserRejected(_)), "{err}");
    }
    let rejections: Vec<_> = server
        .security_events()
        .into_iter()
        .filter_map(|event| match event.kind {
            SecurityEventKind::UserRejected { user_name, reason } => Some((user_name, reason)),
            _ => None,
        })
        .collect();
    assert_eq!(
        rejections,
        [
            ("operator".to_owned(), UserRejection::WrongPassword),
            ("guest".to_owned(), UserRejection::UnknownUser),
        ]
    );

    for policy in SecurityPolicy::SECURE {
        for mode in MessageSecurityMode::SECURE {
            let client = OpcUaClient::connect_with(
                config
                    .clone()
                    .with_security(policy, mode)
                    .with_user_name("operator", "secret"),
            )
            .await
            .unwrap_or_else(|err| panic!("{policy} / {mode} で接続できる: {err}"));
            let address: NodeAddress = "nsu=urn:banto:opcua:tags;s=PLC2.Cell1.Setpoint"
                .parse()
                .expect("NodeAddress");
            let statuses = client
                .write_batch(&[(address, ScalarValue::Double(1.5))])
                .await
                .expect("書く");
            assert!(statuses[0].is_good(), "{statuses:?}");
            assert_eq!(
                received.recv().await.expect("主体"),
                WritePrincipal::User("operator".to_owned())
            );
            client.disconnect().await.expect("切断する");
        }
    }
    let event = server.security_events().pop().expect("記録がある");
    assert_eq!(
        event.kind,
        SecurityEventKind::UserAccepted {
            user_name: "operator".to_owned()
        }
    );
    assert_eq!(event.security_policy, SecurityPolicy::Aes256Sha256RsaPss);
    assert_eq!(event.security_mode, MessageSecurityMode::SignAndEncrypt);
    server.shutdown().await.expect("停止する");
}

#[tokio::test]
async fn inconsistent_security_settings_are_rejected_before_bind() {
    for config in [
        ServerConfig::default().with_anonymous(false),
        ServerConfig::default()
            .with_none_endpoint(true)
            .with_anonymous(false),
        ServerConfig::default().with_security_policies([SecurityPolicy::None]),
    ] {
        let err = OpcUaServer::start(config.with_port(0))
            .await
            .expect_err("成り立たない設定");
        assert!(
            matches!(err, banto_opcua_server::Error::InvalidSecurity(_)),
            "{err}"
        );
    }
}
