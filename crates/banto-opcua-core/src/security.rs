//! 通信の保護と利用者の認証の語彙（ADR-0009）。
//!
//! サーバーとクライアントの設定、エンドポイントの一覧、証明書の信頼の一覧、
//! 利用者名の照合で使う。プロトコル実装の型は持たない。

use std::{fmt, time::SystemTime};

/// SecurityPolicy。署名と暗号化の算法の組。
///
/// 開けるのは Part 7 で廃止扱いでないものだけ。Basic128Rsa15 と Basic256 は扱わない。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[non_exhaustive]
pub enum SecurityPolicy {
    /// 署名も暗号化もしない。
    None,
    /// `Basic256Sha256`。
    Basic256Sha256,
    /// `Aes128_Sha256_RsaOaep`。
    Aes128Sha256RsaOaep,
    /// `Aes256_Sha256_RsaPss`。
    Aes256Sha256RsaPss,
}

impl SecurityPolicy {
    /// サーバーが既定で開く、署名と暗号化をする SecurityPolicy。
    pub const SECURE: [Self; 3] = [
        Self::Basic256Sha256,
        Self::Aes128Sha256RsaOaep,
        Self::Aes256Sha256RsaPss,
    ];

    /// SecurityPolicy の URI（`http://opcfoundation.org/UA/SecurityPolicy#...`）。
    pub const fn uri(self) -> &'static str {
        match self {
            Self::None => "http://opcfoundation.org/UA/SecurityPolicy#None",
            Self::Basic256Sha256 => "http://opcfoundation.org/UA/SecurityPolicy#Basic256Sha256",
            Self::Aes128Sha256RsaOaep => {
                "http://opcfoundation.org/UA/SecurityPolicy#Aes128_Sha256_RsaOaep"
            }
            Self::Aes256Sha256RsaPss => {
                "http://opcfoundation.org/UA/SecurityPolicy#Aes256_Sha256_RsaPss"
            }
        }
    }

    /// URI から引く。扱わない SecurityPolicy（廃止扱いのものを含む）は `None` を返す。
    pub fn from_uri(uri: &str) -> Option<Self> {
        [
            Self::None,
            Self::Basic256Sha256,
            Self::Aes128Sha256RsaOaep,
            Self::Aes256Sha256RsaPss,
        ]
        .into_iter()
        .find(|policy| policy.uri() == uri)
    }
}

impl fmt::Display for SecurityPolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let uri = self.uri();
        f.write_str(uri.rsplit_once('#').map_or(uri, |(_, name)| name))
    }
}

/// MessageSecurityMode。Part 4 の 3 つで閉じている。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum MessageSecurityMode {
    /// 保護しない。SecurityPolicy `None` とだけ組む。
    None,
    /// 署名する。
    Sign,
    /// 署名して暗号化する。
    SignAndEncrypt,
}

impl MessageSecurityMode {
    /// サーバーが既定で開く MessageSecurityMode。
    pub const SECURE: [Self; 2] = [Self::Sign, Self::SignAndEncrypt];
}

impl fmt::Display for MessageSecurityMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::None => "None",
            Self::Sign => "Sign",
            Self::SignAndEncrypt => "SignAndEncrypt",
        })
    }
}

/// 利用者のトークンの種類（UserTokenType）。Part 4 の 4 つで閉じている。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum UserTokenType {
    /// Anonymous。
    Anonymous,
    /// 利用者名とパスワード。
    UserName,
    /// X.509 証明書。本ライブラリは受け付けないが、相手の一覧には現れる。
    Certificate,
    /// 外部で発行されたトークン。本ライブラリは受け付けないが、相手の一覧には現れる。
    IssuedToken,
}

/// 秘密の文字列。`Debug` に中身を出さない。
#[derive(Clone, PartialEq, Eq)]
pub struct Password(String);

impl Password {
    pub fn new(password: impl Into<String>) -> Self {
        Self(password.into())
    }

    /// 中身。ログに出さないこと。
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Password {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Password(****)")
    }
}

impl From<&str> for Password {
    fn from(value: &str) -> Self {
        Self::new(value)
    }
}

impl From<String> for Password {
    fn from(value: String) -> Self {
        Self::new(value)
    }
}

/// サーバーが利用者の照合（[`UserNameLogin`] を受ける関数）へ渡す、1 回のログイン。
///
/// 利用者名は空でない。パスワードは暗号化されたチャネルで受けたものに限る
/// （SecurityPolicy `None` のエンドポイントでは UserName を受け付けない）。
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct UserNameLogin {
    /// 利用者名。
    pub user_name: String,
    /// パスワード。
    pub password: Password,
    /// セッションを開いたエンドポイントの SecurityPolicy。
    pub security_policy: SecurityPolicy,
    /// セッションを開いたエンドポイントの MessageSecurityMode。
    pub security_mode: MessageSecurityMode,
}

impl UserNameLogin {
    pub fn new(
        user_name: impl Into<String>,
        password: Password,
        security_policy: SecurityPolicy,
        security_mode: MessageSecurityMode,
    ) -> Self {
        Self {
            user_name: user_name.into(),
            password,
            security_policy,
            security_mode,
        }
    }
}

/// 利用者の照合が断った理由。クライアントへは区別せずに
/// `BadUserAccessDenied` を返し、理由はサーバーの記録（セキュリティの出来事）にだけ残す。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum UserRejection {
    /// 利用者名が登録されていない。
    UnknownUser,
    /// パスワードが違う。
    WrongPassword,
    /// 利用者は居るが、この接続を許していない（無効化、権限なしなど）。
    NotPermitted,
    /// 照合が時間内に終わらなかった。サーバーが付ける。
    TimedOut,
    /// 照合そのものが失敗した（設定の読み込みの失敗など）。
    Internal(String),
}

impl fmt::Display for UserRejection {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownUser => f.write_str("利用者名が登録されていません"),
            Self::WrongPassword => f.write_str("パスワードが違います"),
            Self::NotPermitted => f.write_str("この利用者には接続を許していません"),
            Self::TimedOut => f.write_str("利用者の照合が時間切れになりました"),
            Self::Internal(reason) => write!(f, "利用者の照合に失敗しました: {reason}"),
        }
    }
}

/// 証明書の、利用する側が画面に出すための要約。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct CertificateInfo {
    /// DER の SHA-1 を大文字の 16 進で並べた拇印。信頼の一覧で証明書を指すのに使う。
    pub thumbprint: String,
    /// Subject の識別名。
    pub subject: String,
    /// 有効期間の始まり。読めなければ `None`。
    pub not_before: Option<SystemTime>,
    /// 有効期間の終わり。読めなければ `None`。
    pub not_after: Option<SystemTime>,
}

impl CertificateInfo {
    pub fn new(
        thumbprint: impl Into<String>,
        subject: impl Into<String>,
        not_before: Option<SystemTime>,
        not_after: Option<SystemTime>,
    ) -> Self {
        Self {
            thumbprint: thumbprint.into(),
            subject: subject.into(),
            not_before,
            not_after,
        }
    }

    /// `now` が有効期間に入っているか。期間を読めない証明書は有効としない。
    pub fn is_valid_at(&self, now: SystemTime) -> bool {
        matches!(
            (self.not_before, self.not_after),
            (Some(start), Some(end)) if start <= now && now <= end
        )
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[test]
    fn policy_uris_round_trip_and_reject_deprecated_policies() {
        for policy in [SecurityPolicy::None]
            .into_iter()
            .chain(SecurityPolicy::SECURE)
        {
            assert_eq!(SecurityPolicy::from_uri(policy.uri()), Some(policy));
        }
        assert_eq!(
            SecurityPolicy::from_uri("http://opcfoundation.org/UA/SecurityPolicy#Basic128Rsa15"),
            None
        );
        assert_eq!(
            SecurityPolicy::Aes128Sha256RsaOaep.to_string(),
            "Aes128_Sha256_RsaOaep"
        );
    }

    #[test]
    fn password_is_not_printed() {
        let login = UserNameLogin::new(
            "operator",
            Password::new("secret"),
            SecurityPolicy::Basic256Sha256,
            MessageSecurityMode::SignAndEncrypt,
        );
        assert!(!format!("{login:?}").contains("secret"));
        assert_eq!(login.password.expose(), "secret");
    }

    #[test]
    fn certificate_validity_needs_both_bounds() {
        let start = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000);
        let end = start + Duration::from_secs(1_000);
        let info = CertificateInfo::new("AB", "CN=x", Some(start), Some(end));
        assert!(info.is_valid_at(start + Duration::from_secs(1)));
        assert!(!info.is_valid_at(end + Duration::from_secs(1)));
        assert!(!info.is_valid_at(start - Duration::from_secs(1)));
        let unknown = CertificateInfo::new("AB", "CN=x", None, Some(end));
        assert!(!unknown.is_valid_at(start));
    }
}
