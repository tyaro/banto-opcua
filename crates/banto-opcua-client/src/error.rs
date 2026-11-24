use banto_opcua_core::{
    CertificateInfo, MessageSecurityMode, SecurityPolicy, StatusCode, UserTokenType,
};

/// サーバーの証明書を信頼の一覧に入れていても通らなかった理由。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum CertificateProblem {
    /// 有効期間の外（期限切れ、またはまだ始まっていない）。
    TimeInvalid,
    /// 接続に使ったホスト名か IP アドレスが、証明書の SubjectAltName に無い。
    HostNameMismatch,
    /// サーバーの ApplicationUri が、証明書の SubjectAltName の URI と違う。
    ApplicationUriMismatch,
}

impl std::fmt::Display for CertificateProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::TimeInvalid => "有効期間の外です",
            Self::HostNameMismatch => "接続先のホスト名が証明書に載っていません",
            Self::ApplicationUriMismatch => "ApplicationUri が証明書と違います",
        })
    }
}

/// クライアントの接続と Read / Write で起きる失敗。
///
/// [`crate::ConnectedClient::read_batch`] の `Err` と [`crate::WriteFailure::error`] は
/// どれも接続ごとの失敗（ノード単位の失敗は結果の StatusCode）。
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// 鍵と証明書、信頼の一覧の置き場（一時または `pki_dir`）を用意できなかった。
    #[error("OPC UA クライアントの証明書の置き場を用意できませんでした: {0}")]
    Pki(#[source] std::io::Error),
    /// クライアント設定の検証に失敗した。
    #[error("OPC UA クライアントの構築に失敗しました: {0}")]
    Build(String),
    /// サーバーに、設定の SecurityPolicy・MessageSecurityMode・利用者のトークンの
    /// 組を受け付けるエンドポイントが無い。選べる組は [`crate::OpcUaClient::endpoints`]。
    #[error(
        "サーバーに {security_policy} / {security_mode} で {token_type:?} を受け付けるエンドポイントがありません"
    )]
    EndpointNotFound {
        /// 求めた SecurityPolicy。
        security_policy: SecurityPolicy,
        /// 求めた MessageSecurityMode。
        security_mode: MessageSecurityMode,
        /// 求めた利用者のトークンの種類。
        token_type: UserTokenType,
    },
    /// サーバーの証明書を信頼していない（初めて会ったときを含む）。証明書は
    /// 拒否の一覧に置いてある。利用する側が拇印を確かめて
    /// [`crate::TrustList::trust`] で信頼に移せば、次の接続から通る（ADR-0009）。
    #[error("サーバーの証明書を信頼していません（拇印 {}、{}）", certificate.thumbprint, certificate.subject)]
    ServerCertificateUntrusted {
        /// サーバーの証明書。
        certificate: CertificateInfo,
    },
    /// サーバーの証明書を信頼しているが、検査を通らなかった。
    #[error("サーバーの証明書が使えません: {problem}（拇印 {}）", certificate.thumbprint)]
    ServerCertificateInvalid {
        /// サーバーの証明書。
        certificate: CertificateInfo,
        /// 通らなかった理由。
        problem: CertificateProblem,
    },
    /// サーバーがこちらの証明書を受け付けなかった。サーバーの管理者が信頼の一覧へ
    /// 入れるまで通らない（初めて接続したときを含む）。証明書が期限切れなどのときも、
    /// サーバーは同じ扱いで断る。
    #[error("サーバーがこちらの証明書を受け付けませんでした: {0}")]
    ClientCertificateRejected(StatusCode),
    /// サーバーが利用者（Anonymous、利用者名とパスワード）を受け付けなかった。
    #[error("サーバーが利用者を受け付けませんでした: {0}")]
    UserRejected(StatusCode),
    /// セッション確立前の失敗、または接続待ちの時間切れ。
    #[error("OPC UA サーバーへ接続できませんでした: {0}")]
    Connect(String),
    /// 接続が切れている（自動再接続のときは張り直している最中）。
    #[error("OPC UA サーバーとの接続が切れています")]
    Disconnected,
    /// 購読や MonitoredItem を作れなかった、または購読を失った（ADR-0006）。
    #[error("OPC UA の購読に失敗しました: {0}")]
    Subscribe(String),
    /// Write の要求が拒否された、または結果を受け取れなかった。
    #[error("OPC UA の Write に失敗しました: {0}")]
    Write(String),
    /// Read サービスが失敗した、または値が想定と違った。
    #[error("OPC UA の Read に失敗しました: {0}")]
    Read(String),
}
