//! banto OPC UA の PKI の置き場（ADR-0003、ADR-0009）。
//!
//! サーバーとクライアントは、それぞれ 1 つのディレクトリに次を置く。
//!
//! ```text
//! <pki_dir>/
//!   own/cert.der         自分のアプリケーションインスタンス証明書（自己署名）
//!   private/private.pem  その秘密鍵（Unix では 0600、親は 0700）
//!   trusted/             信頼する相手の証明書
//!   rejected/            信頼していない相手の証明書。初めて来た証明書もここに置かれる
//! ```
//!
//! 相手の証明書の判断は async-opcua が接続のたびにこのディレクトリを読んで行う。
//! [`TrustList`] でファイルを動かせば、次の接続から効く（再起動は要らない）。
//! 公開 API に async-opcua の型は出さない（ADR-0001）。

mod info;
mod own;
mod trust;

pub use own::{ApplicationIdentity, CERTIFICATE_PATH, PRIVATE_KEY_PATH, prepare_own_certificate};
pub use trust::{TrustList, TrustState, TrustedCertificate};

#[doc(inline)]
pub use banto_opcua_core::CertificateInfo;

/// DER の証明書を読み、画面に出す要約にする。読めなければ `InvalidData`。
pub fn certificate_info(der: &[u8]) -> std::io::Result<CertificateInfo> {
    let cert = opcua::crypto::X509::from_der(der).map_err(|_| info::invalid_certificate())?;
    Ok(info::summarize(&cert))
}
