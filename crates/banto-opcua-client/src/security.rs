//! 通信の保護、利用者、サーバーの証明書の信頼（ADR-0009）。
//!
//! 接続の前に GetEndpoints で相手のエンドポイントを読み、設定に合うものへ直接つなぐ。
//! 断られたときは、サーバーの証明書を自分の信頼の一覧と照らして、どちらの側の
//! 何が理由かを [`Error`] の変種で返す。

use std::{
    path::{Path, PathBuf},
    time::SystemTime,
};

use banto_opcua_core::{CertificateInfo, MessageSecurityMode, SecurityPolicy, UserTokenType};
use banto_opcua_pki::{TrustList, TrustState, certificate_info};
use opcua::{
    client::{Client, IdentityToken},
    core::comms::url::hostname_from_url,
    crypto::{SecurityPolicy as UaSecurityPolicy, X509},
    types::{
        EndpointDescription, MessageSecurityMode as UaMessageSecurityMode,
        StatusCode as UaStatusCode, UserTokenType as UaUserTokenType,
    },
};

use crate::{
    ClientConfig, ClientIdentity, Error, convert::core_status_code, error::CertificateProblem,
};

/// サーバーが広告する 1 つのエンドポイント。接続の画面で選ばせるのに使う。
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct EndpointInfo {
    /// サーバーが広告した URL（接続には設定の URL のホストを使う）。
    pub endpoint_url: String,
    /// SecurityPolicy。本ライブラリが扱わないもの（廃止扱いのものを含む）は `None`。
    pub security_policy: Option<SecurityPolicy>,
    /// SecurityPolicy の URI そのもの。
    pub security_policy_uri: String,
    /// MessageSecurityMode。値が不正なら `None`。
    pub security_mode: Option<MessageSecurityMode>,
    /// 受け付ける利用者のトークンの種類。
    pub user_token_types: Vec<UserTokenType>,
    /// サーバーが付けた安全さの順位。大きいほど強い。
    pub security_level: u8,
    /// サーバーの ApplicationUri。
    pub server_application_uri: String,
    /// サーバーの証明書。`None` のエンドポイントでは無いことがある。
    pub server_certificate: Option<CertificateInfo>,
}

/// 鍵と証明書、信頼の一覧の置き場。一時ディレクトリは切断で消える。
#[derive(Debug)]
pub(crate) enum ClientPki {
    Temporary(tempfile::TempDir),
    Persistent(PathBuf),
}

impl ClientPki {
    pub(crate) fn prepare(dir: Option<&Path>) -> Result<Self, Error> {
        match dir {
            Some(dir) => Ok(Self::Persistent(dir.to_path_buf())),
            None => tempfile::tempdir().map(Self::Temporary).map_err(Error::Pki),
        }
    }

    pub(crate) fn path(&self) -> &Path {
        match self {
            Self::Temporary(dir) => dir.path(),
            Self::Persistent(dir) => dir,
        }
    }
}

pub(crate) fn ua_policy(policy: SecurityPolicy) -> UaSecurityPolicy {
    UaSecurityPolicy::from_uri(policy.uri())
}

pub(crate) fn ua_mode(mode: MessageSecurityMode) -> UaMessageSecurityMode {
    match mode {
        MessageSecurityMode::None => UaMessageSecurityMode::None,
        MessageSecurityMode::Sign => UaMessageSecurityMode::Sign,
        MessageSecurityMode::SignAndEncrypt => UaMessageSecurityMode::SignAndEncrypt,
    }
}

fn core_mode(mode: UaMessageSecurityMode) -> Option<MessageSecurityMode> {
    match mode {
        UaMessageSecurityMode::None => Some(MessageSecurityMode::None),
        UaMessageSecurityMode::Sign => Some(MessageSecurityMode::Sign),
        UaMessageSecurityMode::SignAndEncrypt => Some(MessageSecurityMode::SignAndEncrypt),
        _ => None,
    }
}

fn core_token_type(token_type: UaUserTokenType) -> UserTokenType {
    match token_type {
        UaUserTokenType::Anonymous => UserTokenType::Anonymous,
        UaUserTokenType::UserName => UserTokenType::UserName,
        UaUserTokenType::Certificate => UserTokenType::Certificate,
        UaUserTokenType::IssuedToken => UserTokenType::IssuedToken,
    }
}

pub(crate) fn identity_token(identity: &ClientIdentity) -> IdentityToken {
    match identity {
        ClientIdentity::Anonymous => IdentityToken::Anonymous,
        ClientIdentity::UserName {
            user_name,
            password,
        } => IdentityToken::new_user_name(user_name.clone(), password.expose().to_owned()),
    }
}

pub(crate) fn endpoint_info(endpoint: &EndpointDescription) -> EndpointInfo {
    let security_policy_uri = endpoint.security_policy_uri.as_ref().to_owned();
    EndpointInfo {
        endpoint_url: endpoint.endpoint_url.as_ref().to_owned(),
        security_policy: SecurityPolicy::from_uri(&security_policy_uri),
        security_policy_uri,
        security_mode: core_mode(endpoint.security_mode),
        user_token_types: endpoint
            .user_identity_tokens
            .iter()
            .flatten()
            .map(|policy| core_token_type(policy.token_type))
            .collect(),
        security_level: endpoint.security_level,
        server_application_uri: endpoint.server.application_uri.as_ref().to_owned(),
        server_certificate: server_certificate(endpoint)
            .and_then(|der| certificate_info(&der).ok()),
    }
}

/// 設定の SecurityPolicy・MessageSecurityMode・利用者に合うエンドポイントを選ぶ。
/// URL のホストは、サーバーが広告したものではなく設定のものに置き換える。
pub(crate) fn select_endpoint(
    endpoints: &[EndpointDescription],
    config: &ClientConfig,
) -> Result<EndpointDescription, Error> {
    let token_type = config.identity.token_type();
    Client::find_matching_endpoint(
        endpoints,
        &config.endpoint_url,
        ua_policy(config.security_policy),
        ua_mode(config.security_mode),
    )
    .filter(|endpoint| {
        endpoint
            .user_identity_tokens
            .iter()
            .flatten()
            .any(|policy| core_token_type(policy.token_type) == token_type)
    })
    .ok_or(Error::EndpointNotFound {
        security_policy: config.security_policy,
        security_mode: config.security_mode,
        token_type,
    })
}

/// セッションを開く前に、サーバーの証明書を自分の信頼の一覧と照らす。
///
/// サーバーはこちらの証明書を CreateSession の要求の時点で確かめ、こちらはサーバーの
/// 証明書をその応答で確かめる。どちらの拒否も同じ StatusCode で返るので、接続してから
/// では理由を分けられず、両方が初対面ならサーバーの拒否が先に出る。そこで GetEndpoints で
/// 受けた証明書を先に確かめる。
///
/// - 一覧に無ければ拒否の一覧へ置き（保留）、[`Error::ServerCertificateUntrusted`]。
///   [`ClientConfig::trust_all_server_certificates`] なら信頼の一覧へ置いて続ける。
/// - 拒否の一覧にあれば [`Error::ServerCertificateUntrusted`]。
/// - 信頼していても有効期間・ホスト名・ApplicationUri が合わなければ
///   [`Error::ServerCertificateInvalid`]。
///
/// ここで通した証明書も、接続のときに async-opcua が改めて確かめる（チャネルの証明書と
/// 同じであることを含む）。
pub(crate) fn check_server_certificate(
    endpoint: &EndpointDescription,
    config: &ClientConfig,
    pki_dir: &Path,
) -> Result<(), Error> {
    if config.security_policy == SecurityPolicy::None {
        return Ok(());
    }
    let der = server_certificate(endpoint)
        .ok_or_else(|| Error::Connect("サーバーが証明書を示しませんでした".to_owned()))?;
    let x509 = X509::from_der(&der)
        .map_err(|_| Error::Connect("サーバーの証明書を読めません".to_owned()))?;
    let list = TrustList::new(pki_dir);
    let (certificate, state) = if config.trust_all_server_certificates {
        let certificate = list.add_trusted(&der).map_err(Error::Pki)?;
        (certificate, TrustState::Trusted)
    } else {
        list.add_pending(&der).map_err(Error::Pki)?
    };
    if state != TrustState::Trusted {
        return Err(Error::ServerCertificateUntrusted { certificate });
    }
    let problem = if !certificate.is_valid_at(SystemTime::now()) {
        Some(CertificateProblem::TimeInvalid)
    } else if hostname_from_url(&config.endpoint_url)
        .map_or(true, |host| x509.is_hostname_valid(&host).is_err())
    {
        Some(CertificateProblem::HostNameMismatch)
    } else if x509
        .is_application_uri_valid(endpoint.server.application_uri.as_ref())
        .is_err()
    {
        Some(CertificateProblem::ApplicationUriMismatch)
    } else {
        None
    };
    match problem {
        Some(problem) => Err(Error::ServerCertificateInvalid {
            certificate,
            problem,
        }),
        None => Ok(()),
    }
}

/// セッションを開けなかった理由を分ける。
///
/// サーバーの証明書は [`check_server_certificate`] で通してあるので、証明書の失敗は
/// サーバーがこちらの証明書を断ったものとみなす（GetEndpoints と接続の間にサーバーの
/// 証明書が替わったときだけ、この見立ては外れる）。
pub(crate) fn classify_failure(status: UaStatusCode, config: &ClientConfig) -> Error {
    if matches!(
        status,
        UaStatusCode::BadIdentityTokenRejected
            | UaStatusCode::BadIdentityTokenInvalid
            | UaStatusCode::BadUserAccessDenied
            | UaStatusCode::BadUserSignatureInvalid
    ) {
        return Error::UserRejected(core_status_code(status));
    }
    let certificate_failure = matches!(
        status,
        UaStatusCode::BadSecurityChecksFailed
            | UaStatusCode::BadCertificateInvalid
            | UaStatusCode::BadCertificateTimeInvalid
            | UaStatusCode::BadCertificateIssuerTimeInvalid
            | UaStatusCode::BadCertificateHostNameInvalid
            | UaStatusCode::BadCertificateUriInvalid
            | UaStatusCode::BadCertificateUseNotAllowed
            | UaStatusCode::BadCertificateIssuerUseNotAllowed
            | UaStatusCode::BadCertificateUntrusted
            | UaStatusCode::BadCertificateRevocationUnknown
            | UaStatusCode::BadCertificateIssuerRevocationUnknown
            | UaStatusCode::BadCertificateRevoked
            | UaStatusCode::BadCertificateIssuerRevoked
            | UaStatusCode::BadCertificatePolicyCheckFailed
    );
    if certificate_failure && config.security_policy != SecurityPolicy::None {
        Error::ClientCertificateRejected(core_status_code(status))
    } else {
        Error::Connect(format!(
            "セッションが確立しませんでした: {}",
            core_status_code(status)
        ))
    }
}

fn server_certificate(endpoint: &EndpointDescription) -> Option<Vec<u8>> {
    endpoint
        .server_certificate
        .value
        .as_ref()
        .filter(|der| !der.is_empty())
        .cloned()
}

#[cfg(test)]
mod tests {
    use opcua::types::{UAString, UserTokenPolicy};

    use super::*;

    fn endpoint(policy: SecurityPolicy, mode: MessageSecurityMode) -> EndpointDescription {
        EndpointDescription {
            endpoint_url: "opc.tcp://server.example:4840/".into(),
            security_mode: ua_mode(mode),
            security_policy_uri: UAString::from(policy.uri()),
            user_identity_tokens: Some(vec![UserTokenPolicy {
                token_type: UaUserTokenType::Anonymous,
                ..Default::default()
            }]),
            security_level: 14,
            ..Default::default()
        }
    }

    #[test]
    fn endpoint_selection_needs_the_token_type() {
        let endpoints = [
            endpoint(SecurityPolicy::None, MessageSecurityMode::None),
            endpoint(
                SecurityPolicy::Basic256Sha256,
                MessageSecurityMode::SignAndEncrypt,
            ),
        ];
        let config = ClientConfig::new("opc.tcp://127.0.0.1:4840/")
            .with_security(
                SecurityPolicy::Basic256Sha256,
                MessageSecurityMode::SignAndEncrypt,
            )
            .with_pki_dir("pki");
        let selected = select_endpoint(&endpoints, &config).expect("Anonymous で選べる");
        assert_eq!(
            selected.endpoint_url.as_ref(),
            "opc.tcp://127.0.0.1:4840/",
            "ホストは設定のものにする"
        );
        let err = select_endpoint(&endpoints, &config.with_user_name("u", "p"))
            .expect_err("UserName は広告されていない");
        assert!(matches!(
            err,
            Error::EndpointNotFound {
                token_type: UserTokenType::UserName,
                ..
            }
        ));
    }

    #[test]
    fn endpoint_info_maps_known_and_unknown_policies() {
        let info = endpoint_info(&endpoint(
            SecurityPolicy::Aes256Sha256RsaPss,
            MessageSecurityMode::Sign,
        ));
        assert_eq!(
            info.security_policy,
            Some(SecurityPolicy::Aes256Sha256RsaPss)
        );
        assert_eq!(info.security_mode, Some(MessageSecurityMode::Sign));
        assert_eq!(info.user_token_types, [UserTokenType::Anonymous]);
        assert_eq!(info.server_certificate, None);
        let mut legacy = endpoint(SecurityPolicy::None, MessageSecurityMode::Sign);
        legacy.security_policy_uri =
            UAString::from("http://opcfoundation.org/UA/SecurityPolicy#Basic128Rsa15");
        assert_eq!(endpoint_info(&legacy).security_policy, None);
    }

    #[test]
    fn failures_are_split_by_side() {
        let plain = ClientConfig::new("opc.tcp://127.0.0.1:4840/");
        let secure = plain.clone().with_security(
            SecurityPolicy::Basic256Sha256,
            MessageSecurityMode::SignAndEncrypt,
        );
        assert!(matches!(
            classify_failure(UaStatusCode::BadUserAccessDenied, &plain),
            Error::UserRejected(_)
        ));
        assert!(matches!(
            classify_failure(UaStatusCode::BadCertificateUntrusted, &plain),
            Error::Connect(_)
        ));
        assert!(matches!(
            classify_failure(UaStatusCode::BadSecurityChecksFailed, &secure),
            Error::ClientCertificateRejected(_)
        ));
        assert!(matches!(
            classify_failure(UaStatusCode::BadTimeout, &secure),
            Error::Connect(_)
        ));
    }

    /// サーバーの証明書の要約と、それを載せたエンドポイント。
    fn server(root: &Path, uri: &str) -> (EndpointDescription, banto_opcua_core::CertificateInfo) {
        let dir = root.join("server");
        let info = banto_opcua_pki::prepare_own_certificate(
            &dir,
            &banto_opcua_pki::ApplicationIdentity::new("server", "urn:banto:test:server"),
        )
        .expect("サーバーの証明書を作る");
        let der = std::fs::read(dir.join(banto_opcua_pki::CERTIFICATE_PATH)).expect("DER");
        let mut endpoint = endpoint(
            SecurityPolicy::Basic256Sha256,
            MessageSecurityMode::SignAndEncrypt,
        );
        endpoint.server_certificate = opcua::types::ByteString::from(der);
        endpoint.server.application_uri = UAString::from(uri);
        (endpoint, info)
    }

    #[test]
    fn server_certificate_is_pending_until_trusted_then_checked() {
        let root = tempfile::tempdir().expect("試験用ディレクトリ");
        let pki = root.path().join("client");
        let (endpoint, info) = server(root.path(), "urn:banto:test:server");
        let config = ClientConfig::new("opc.tcp://127.0.0.1:4840/")
            .with_security(
                SecurityPolicy::Basic256Sha256,
                MessageSecurityMode::SignAndEncrypt,
            )
            .with_pki_dir(&pki);

        let err = check_server_certificate(&endpoint, &config, &pki).expect_err("初対面");
        assert!(
            matches!(&err, Error::ServerCertificateUntrusted { certificate } if *certificate == info),
            "{err}"
        );
        assert!(
            check_server_certificate(&endpoint, &config, &pki).is_err(),
            "保留のまま"
        );
        TrustList::new(&pki)
            .trust(&info.thumbprint)
            .expect("信頼する");
        check_server_certificate(&endpoint, &config, &pki).expect("信頼すれば通る");

        let elsewhere = config.clone();
        let elsewhere = ClientConfig {
            endpoint_url: "opc.tcp://plc.invalid:4840/".to_owned(),
            ..elsewhere
        };
        assert!(matches!(
            check_server_certificate(&endpoint, &elsewhere, &pki),
            Err(Error::ServerCertificateInvalid {
                problem: CertificateProblem::HostNameMismatch,
                ..
            })
        ));
        let (renamed, _) = server(root.path(), "urn:banto:test:other");
        assert!(matches!(
            check_server_certificate(&renamed, &config, &pki),
            Err(Error::ServerCertificateInvalid {
                problem: CertificateProblem::ApplicationUriMismatch,
                ..
            })
        ));
    }

    #[test]
    fn trust_all_accepts_unknown_server_certificates() {
        let root = tempfile::tempdir().expect("試験用ディレクトリ");
        let pki = root.path().join("client");
        let (endpoint, info) = server(root.path(), "urn:banto:test:server");
        let config = ClientConfig::new("opc.tcp://127.0.0.1:4840/")
            .with_security(SecurityPolicy::Basic256Sha256, MessageSecurityMode::Sign)
            .with_trust_all_server_certificates(true);
        check_server_certificate(&endpoint, &config, &pki).expect("信頼して通る");
        assert_eq!(
            TrustList::new(&pki)
                .state_of(&info.thumbprint)
                .expect("一覧"),
            Some(TrustState::Trusted)
        );
    }
}
