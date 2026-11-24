//! 利用者の認証と、受け付けた・断った接続の記録（ADR-0009）。
//!
//! async-opcua の `AuthManager` を実装し、Anonymous の可否と、利用者名の照合を
//! 利用者が渡す [`UserAuthenticator`] へ委ねる。banto-opcua は資格情報を持たない。

use std::{
    collections::VecDeque,
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
    time::{Duration, SystemTime},
};

use async_trait::async_trait;
use banto_opcua_core::{
    MessageSecurityMode, Password, SecurityPolicy, UserNameLogin, UserRejection,
};
use opcua::{
    crypto::SecurityPolicy as UaSecurityPolicy,
    server::{
        ServerEndpoint,
        authenticator::{
            AuthManager, Password as UaPassword, UserToken, user_pass_security_policy_id,
        },
    },
    types::{
        Error as UaError, MessageSecurityMode as UaMessageSecurityMode, StatusCode, UAString,
        UserTokenPolicy, UserTokenType,
    },
};

/// [`UserAuthenticator::authenticate`] が返す future。
pub type AuthenticateFuture =
    Pin<Box<dyn Future<Output = Result<(), UserRejection>> + Send + 'static>>;

/// 利用者名とパスワードを照合する、利用者の処理（ADR-0009）。
///
/// 渡すと、SecurityPolicy が `None` でないエンドポイントで UserName のトークンを
/// 受け付ける。`None` のエンドポイントでは平文のパスワードになるので受け付けない。
/// 照合は ActivateSession の中で呼ばれ、10 秒で終わらなければ
/// [`UserRejection::TimedOut`] として断る。クライアントへは理由を区別せずに
/// `BadUserAccessDenied` を返す。
pub trait UserAuthenticator: Send + Sync + 'static {
    /// 1 回のログインを照合する。通すなら `Ok(())`。
    fn authenticate(&self, login: UserNameLogin) -> AuthenticateFuture;
}

/// 照合を待つ上限。ActivateSession の応答を返せないまま待ち続けない。
const AUTHENTICATE_TIMEOUT: Duration = Duration::from_secs(10);

/// async-opcua が Anonymous のトークンに求める policy_id。async-opcua の値と同じにする。
const ANONYMOUS_POLICY_ID: &str = "anonymous";

/// 利用者のトークンの接頭辞。利用者名がどんな文字列でも、async-opcua の Anonymous の
/// 印（`ANONYMOUS`）と取り違えないようにする。
const USER_TOKEN_PREFIX: &str = "user:";

/// 照合を通した UserName のトークンから利用者名を取り出す。Anonymous なら `None`。
pub(crate) fn user_name_of(token: &UserToken) -> Option<&str> {
    token.0.strip_prefix(USER_TOKEN_PREFIX)
}

/// 受け付けた・断ったセッションの記録の 1 件。
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct SecurityEvent {
    /// 記録した時刻。
    pub at: SystemTime,
    /// セッションを開いたエンドポイントの SecurityPolicy。
    pub security_policy: SecurityPolicy,
    /// セッションを開いたエンドポイントの MessageSecurityMode。
    pub security_mode: MessageSecurityMode,
    /// 何が起きたか。
    pub kind: SecurityEventKind,
}

/// [`SecurityEvent`] の中身。
///
/// クライアントの証明書による拒否は、async-opcua が CreateSession の中で行い、ここには
/// 現れない。信頼していない証明書は [`banto_opcua_pki::TrustList`] の拒否の一覧に、
/// 期限切れは [`banto_opcua_core::CertificateInfo::not_after`] に現れる（ADR-0009）。
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SecurityEventKind {
    /// Anonymous のセッションを受け付けた。
    AnonymousAccepted,
    /// Anonymous を許していないので断った。
    AnonymousRejected,
    /// 利用者の照合を通した。
    UserAccepted {
        /// 利用者名。
        user_name: String,
    },
    /// 利用者の照合で断った。
    UserRejected {
        /// 利用者名。
        user_name: String,
        /// 断った理由。
        reason: UserRejection,
    },
    /// UserName を受け付けないエンドポイント（照合の関数が無い、または SecurityPolicy
    /// `None`）へ UserName が来たので断った。
    UserNameNotAccepted {
        /// 利用者名。
        user_name: String,
    },
}

/// 直近の [`SecurityEvent`] を古い順に持つ。上限を超えたら古いものから捨てる。
#[derive(Debug)]
pub(crate) struct SecurityLog {
    events: Mutex<VecDeque<SecurityEvent>>,
    capacity: usize,
}

impl SecurityLog {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            events: Mutex::new(VecDeque::with_capacity(capacity)),
            capacity,
        }
    }

    pub(crate) fn snapshot(&self) -> Vec<SecurityEvent> {
        self.events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .cloned()
            .collect()
    }

    fn push(&self, endpoint: &ServerEndpoint, kind: SecurityEventKind) {
        let event = SecurityEvent {
            at: SystemTime::now(),
            security_policy: core_policy(endpoint.security_policy()),
            security_mode: core_mode(endpoint.message_security_mode()),
            kind,
        };
        let mut events = self
            .events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if events.len() == self.capacity {
            events.pop_front();
        }
        events.push_back(event);
    }
}

/// async-opcua の `AuthManager`。Anonymous の可否と UserName の照合を受け持つ。
pub(crate) struct Gatekeeper {
    pub(crate) allow_anonymous: bool,
    pub(crate) users: Option<Arc<dyn UserAuthenticator>>,
    pub(crate) log: Arc<SecurityLog>,
}

impl Gatekeeper {
    fn accepts_user_name(&self, endpoint: &ServerEndpoint) -> bool {
        self.users.is_some() && endpoint.security_policy() != UaSecurityPolicy::None
    }
}

#[async_trait]
impl AuthManager for Gatekeeper {
    async fn authenticate_anonymous_token(&self, endpoint: &ServerEndpoint) -> Result<(), UaError> {
        if self.allow_anonymous {
            self.log
                .push(endpoint, SecurityEventKind::AnonymousAccepted);
            Ok(())
        } else {
            self.log
                .push(endpoint, SecurityEventKind::AnonymousRejected);
            Err(UaError::new(
                StatusCode::BadIdentityTokenRejected,
                "Anonymous を許していません",
            ))
        }
    }

    async fn authenticate_username_identity_token(
        &self,
        endpoint: &ServerEndpoint,
        username: &str,
        password: &UaPassword,
    ) -> Result<UserToken, UaError> {
        let user_name = username.to_owned();
        let Some(users) = self
            .users
            .as_ref()
            .filter(|_| self.accepts_user_name(endpoint))
        else {
            self.log.push(
                endpoint,
                SecurityEventKind::UserNameNotAccepted { user_name },
            );
            return Err(UaError::new(
                StatusCode::BadIdentityTokenRejected,
                "このエンドポイントは UserName を受け付けません",
            ));
        };
        let login = UserNameLogin::new(
            user_name.clone(),
            Password::new(password.get()),
            core_policy(endpoint.security_policy()),
            core_mode(endpoint.message_security_mode()),
        );
        let result = tokio::time::timeout(AUTHENTICATE_TIMEOUT, users.authenticate(login))
            .await
            .unwrap_or(Err(UserRejection::TimedOut));
        match result {
            Ok(()) => {
                let token = UserToken(format!("{USER_TOKEN_PREFIX}{user_name}"));
                self.log
                    .push(endpoint, SecurityEventKind::UserAccepted { user_name });
                Ok(token)
            }
            Err(reason) => {
                tracing::warn!(user_name, %reason, "OPC UA の利用者の照合で断りました");
                self.log.push(
                    endpoint,
                    SecurityEventKind::UserRejected { user_name, reason },
                );
                Err(UaError::new(
                    StatusCode::BadUserAccessDenied,
                    "利用者名かパスワードが違います",
                ))
            }
        }
    }

    fn user_token_policies(&self, endpoint: &ServerEndpoint) -> Vec<UserTokenPolicy> {
        let mut policies = Vec::with_capacity(2);
        if self.allow_anonymous {
            policies.push(UserTokenPolicy {
                policy_id: UAString::from(ANONYMOUS_POLICY_ID),
                token_type: UserTokenType::Anonymous,
                issued_token_type: UAString::null(),
                issuer_endpoint_url: UAString::null(),
                security_policy_uri: UAString::null(),
            });
        }
        if self.accepts_user_name(endpoint) {
            // パスワードはエンドポイントの SecurityPolicy で暗号化させる（URI は null）。
            policies.push(UserTokenPolicy {
                policy_id: user_pass_security_policy_id(endpoint),
                token_type: UserTokenType::UserName,
                issued_token_type: UAString::null(),
                issuer_endpoint_url: UAString::null(),
                security_policy_uri: UAString::null(),
            });
        }
        policies
    }
}

/// async-opcua の SecurityPolicy を core へ写す。サーバーが開くのは core にあるものだけ。
pub(crate) fn core_policy(policy: UaSecurityPolicy) -> SecurityPolicy {
    SecurityPolicy::from_uri(policy.to_uri()).unwrap_or(SecurityPolicy::None)
}

pub(crate) fn ua_policy(policy: SecurityPolicy) -> UaSecurityPolicy {
    UaSecurityPolicy::from_uri(policy.uri())
}

fn core_mode(mode: UaMessageSecurityMode) -> MessageSecurityMode {
    match mode {
        UaMessageSecurityMode::Sign => MessageSecurityMode::Sign,
        UaMessageSecurityMode::SignAndEncrypt => MessageSecurityMode::SignAndEncrypt,
        _ => MessageSecurityMode::None,
    }
}

pub(crate) fn ua_mode(mode: MessageSecurityMode) -> UaMessageSecurityMode {
    match mode {
        MessageSecurityMode::None => UaMessageSecurityMode::None,
        MessageSecurityMode::Sign => UaMessageSecurityMode::Sign,
        MessageSecurityMode::SignAndEncrypt => UaMessageSecurityMode::SignAndEncrypt,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_tokens_never_look_anonymous() {
        let token = UserToken(format!("{USER_TOKEN_PREFIX}ANONYMOUS"));
        assert!(!token.is_anonymous());
        assert_eq!(user_name_of(&token), Some("ANONYMOUS"));
        assert_eq!(
            user_name_of(&UserToken(
                opcua::server::ANONYMOUS_USER_TOKEN_ID.to_owned()
            )),
            None
        );
    }

    #[test]
    fn policies_round_trip_through_async_opcua() {
        for policy in [SecurityPolicy::None]
            .into_iter()
            .chain(SecurityPolicy::SECURE)
        {
            assert_eq!(core_policy(ua_policy(policy)), policy);
        }
        for mode in [MessageSecurityMode::None]
            .into_iter()
            .chain(MessageSecurityMode::SECURE)
        {
            assert_eq!(core_mode(ua_mode(mode)), mode);
        }
    }

    #[test]
    fn log_keeps_the_newest_events() {
        let log = SecurityLog::new(2);
        let endpoint = ServerEndpoint::new_basic256sha256_sign_encrypt("/", &[]);
        for user_name in ["a", "b", "c"] {
            log.push(
                &endpoint,
                SecurityEventKind::UserAccepted {
                    user_name: user_name.to_owned(),
                },
            );
        }
        let names: Vec<_> = log
            .snapshot()
            .into_iter()
            .map(|event| match event.kind {
                SecurityEventKind::UserAccepted { user_name } => user_name,
                other => panic!("{other:?}"),
            })
            .collect();
        assert_eq!(names, ["b", "c"]);
        assert_eq!(
            log.snapshot()[0].security_policy,
            SecurityPolicy::Basic256Sha256
        );
    }
}
