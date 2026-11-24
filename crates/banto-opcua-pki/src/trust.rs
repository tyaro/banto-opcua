//! 相手の証明書の信頼の一覧（ADR-0009）。

use std::{
    io,
    path::{Path, PathBuf},
};

use banto_opcua_core::CertificateInfo;
use opcua::crypto::{CertificateStore, X509};

use crate::info;

const TRUSTED_DIR: &str = "trusted";
const REJECTED_DIR: &str = "rejected";

/// 一覧での証明書の状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum TrustState {
    /// `trusted/` にある。有効期間の中なら接続を受け付ける。
    Trusted,
    /// `rejected/` にある。初めて来た証明書もここに置かれ、管理者が信頼に移すまで断る。
    Rejected,
}

/// 一覧に載っている 1 枚の証明書。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct TrustedCertificate {
    /// 証明書の要約。期限切れかどうかは [`CertificateInfo::is_valid_at`] で見る。
    pub info: CertificateInfo,
    /// どちらの一覧にあるか。
    pub state: TrustState,
}

/// `pki_dir` の `trusted/` と `rejected/` を読み書きする。
///
/// async-opcua は接続のたびに、相手の証明書が `rejected/` にあれば断り、
/// `trusted/` にあれば有効期間などを確かめて受け付け、どちらにも無ければ
/// `rejected/` に置いて断る。ここでファイルを動かすと次の接続から効く。
/// 証明書は拇印（大文字・小文字は問わない）で指す。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustList {
    dir: PathBuf,
}

impl TrustList {
    /// `pki_dir` の一覧を扱う。ディレクトリはまだ無くてもよい（読むと空、書くと作る）。
    pub fn new(pki_dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: pki_dir.into(),
        }
    }

    /// `pki_dir`。
    pub fn pki_dir(&self) -> &Path {
        &self.dir
    }

    /// 両方の一覧の証明書。信頼するものを先に、それぞれ Subject の順に並べる。
    /// 証明書として読めないファイルは飛ばす。
    pub fn certificates(&self) -> io::Result<Vec<TrustedCertificate>> {
        let mut all = Vec::new();
        for state in [TrustState::Trusted, TrustState::Rejected] {
            let mut listed: Vec<_> = self
                .read_dir(state)?
                .into_iter()
                .map(|(_, cert)| TrustedCertificate {
                    info: info::summarize(&cert),
                    state,
                })
                .collect();
            listed.sort_by(|a, b| a.info.subject.cmp(&b.info.subject));
            all.extend(listed);
        }
        Ok(all)
    }

    /// 拒否の一覧にある証明書を、信頼の一覧へ移す。既に信頼していれば何もしない。
    /// どちらにも無ければ `NotFound`。
    pub fn trust(&self, thumbprint: &str) -> io::Result<CertificateInfo> {
        self.move_to(thumbprint, TrustState::Trusted)
    }

    /// 信頼の一覧にある証明書を、拒否の一覧へ移す。既に拒否していれば何もしない。
    /// どちらにも無ければ `NotFound`。
    pub fn reject(&self, thumbprint: &str) -> io::Result<CertificateInfo> {
        self.move_to(thumbprint, TrustState::Rejected)
    }

    /// 両方の一覧から消す。次にその相手が来たら、初めての証明書として拒否の一覧に置かれる。
    /// どちらにも無ければ `NotFound`。
    pub fn remove(&self, thumbprint: &str) -> io::Result<()> {
        let found = self.find(thumbprint)?;
        if found.is_empty() {
            return Err(not_found(thumbprint));
        }
        for (path, _, _) in found {
            std::fs::remove_file(path)?;
        }
        Ok(())
    }

    /// DER の証明書を信頼の一覧へ置く（相手の証明書を前もって受け取ったとき）。
    /// 拒否の一覧に同じ証明書があれば消す。証明書として読めなければ `InvalidData`。
    pub fn add_trusted(&self, der: &[u8]) -> io::Result<CertificateInfo> {
        let cert = X509::from_der(der).map_err(|_| info::invalid_certificate())?;
        let summary = info::summarize(&cert);
        ensure_dirs(&self.dir)?;
        for (path, _, state) in self.find(&summary.thumbprint)? {
            if state == TrustState::Rejected {
                std::fs::remove_file(path)?;
            }
        }
        std::fs::write(self.file_for(&cert, TrustState::Trusted), der)?;
        Ok(summary)
    }

    /// DER の証明書が、どちらの一覧にも無ければ拒否の一覧へ置く（保留にする）。
    /// 返すのは要約と、置いた後の状態（既に一覧にあれば、その状態のまま変えない）。
    /// 証明書として読めなければ `InvalidData`。
    pub fn add_pending(&self, der: &[u8]) -> io::Result<(CertificateInfo, TrustState)> {
        let cert = X509::from_der(der).map_err(|_| info::invalid_certificate())?;
        let summary = info::summarize(&cert);
        if let Some(state) = self.state_of(&summary.thumbprint)? {
            return Ok((summary, state));
        }
        ensure_dirs(&self.dir)?;
        std::fs::write(self.file_for(&cert, TrustState::Rejected), der)?;
        Ok((summary, TrustState::Rejected))
    }

    /// 拇印の証明書がどちらの一覧にあるか。両方にあれば信頼を返す。どちらにも無ければ `None`。
    pub fn state_of(&self, thumbprint: &str) -> io::Result<Option<TrustState>> {
        let states: Vec<_> = self
            .find(thumbprint)?
            .into_iter()
            .map(|(_, _, state)| state)
            .collect();
        Ok(if states.contains(&TrustState::Trusted) {
            Some(TrustState::Trusted)
        } else {
            states.first().copied()
        })
    }

    fn move_to(&self, thumbprint: &str, target: TrustState) -> io::Result<CertificateInfo> {
        let found = self.find(thumbprint)?;
        let Some((_, cert, _)) = found.first() else {
            return Err(not_found(thumbprint));
        };
        let summary = info::summarize(cert);
        ensure_dirs(&self.dir)?;
        // async-opcua は信頼の一覧のファイル名を証明書から決めて読むので、その名前で置く。
        let destination = self.file_for(cert, target);
        let der = cert.to_der().map_err(|_| info::invalid_certificate())?;
        std::fs::write(&destination, der)?;
        for (path, _, _) in found {
            if path != destination {
                std::fs::remove_file(path)?;
            }
        }
        Ok(summary)
    }

    /// 拇印が一致する証明書のファイル。両方の一覧を見る。
    fn find(&self, thumbprint: &str) -> io::Result<Vec<(PathBuf, X509, TrustState)>> {
        let mut found = Vec::new();
        for state in [TrustState::Trusted, TrustState::Rejected] {
            for (path, cert) in self.read_dir(state)? {
                if info::thumbprint(&cert).eq_ignore_ascii_case(thumbprint.trim()) {
                    found.push((path, cert, state));
                }
            }
        }
        Ok(found)
    }

    fn read_dir(&self, state: TrustState) -> io::Result<Vec<(PathBuf, X509)>> {
        let dir = self.dir.join(dir_name(state));
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let mut certs = Vec::new();
        for entry in entries {
            let path = entry?.path();
            if !path.is_file() {
                continue;
            }
            match CertificateStore::read_cert(&path) {
                Ok(cert) => certs.push((path, cert)),
                Err(reason) => {
                    tracing::debug!(path = %path.display(), %reason, "証明書として読めないファイルを飛ばします");
                }
            }
        }
        Ok(certs)
    }

    fn file_for(&self, cert: &X509, state: TrustState) -> PathBuf {
        self.dir
            .join(dir_name(state))
            .join(CertificateStore::cert_file_name(cert))
    }
}

/// `trusted/` と `rejected/` を作る。async-opcua はどちらかが無いと検証を失敗にする。
pub(crate) fn ensure_dirs(pki_dir: &Path) -> io::Result<()> {
    for name in [TRUSTED_DIR, REJECTED_DIR] {
        std::fs::create_dir_all(pki_dir.join(name))?;
    }
    Ok(())
}

fn dir_name(state: TrustState) -> &'static str {
    match state {
        TrustState::Trusted => TRUSTED_DIR,
        TrustState::Rejected => REJECTED_DIR,
    }
}

fn not_found(thumbprint: &str) -> io::Error {
    io::Error::new(
        io::ErrorKind::NotFound,
        format!("拇印 {thumbprint} の証明書は信頼の一覧にも拒否の一覧にもありません"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ApplicationIdentity, CERTIFICATE_PATH, prepare_own_certificate};

    /// 別のディレクトリで作った証明書の DER と要約。
    fn peer(root: &Path, name: &str) -> (Vec<u8>, CertificateInfo) {
        let dir = root.join(name);
        let summary = prepare_own_certificate(
            &dir,
            &ApplicationIdentity::new(name, format!("urn:banto:test:{name}")),
        )
        .expect("相手の証明書を作る");
        (
            std::fs::read(dir.join(CERTIFICATE_PATH)).expect("DER を読む"),
            summary,
        )
    }

    #[test]
    fn rejected_certificates_move_between_lists() {
        let root = tempfile::tempdir().expect("試験用ディレクトリ");
        let list = TrustList::new(root.path().join("own"));
        assert_eq!(list.certificates().expect("空の一覧"), []);

        let (der, summary) = peer(root.path(), "peer");
        // async-opcua が初めての証明書を置くのと同じ場所・名前。
        ensure_dirs(list.pki_dir()).expect("一覧を作る");
        let cert = X509::from_der(&der).expect("DER");
        std::fs::write(list.file_for(&cert, TrustState::Rejected), &der).expect("拒否に置く");
        let listed = list.certificates().expect("一覧");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].state, TrustState::Rejected);
        assert_eq!(listed[0].info, summary);

        let lower = summary.thumbprint.to_ascii_lowercase();
        assert_eq!(list.trust(&lower).expect("信頼する"), summary);
        assert_eq!(
            list.certificates().expect("一覧")[0].state,
            TrustState::Trusted
        );
        assert!(list.file_for(&cert, TrustState::Trusted).is_file());
        assert!(!list.file_for(&cert, TrustState::Rejected).exists());
        assert_eq!(
            list.trust(&summary.thumbprint).expect("2 回目は何もしない"),
            summary
        );

        list.reject(&summary.thumbprint).expect("拒否へ戻す");
        assert_eq!(
            list.certificates().expect("一覧")[0].state,
            TrustState::Rejected
        );

        list.remove(&summary.thumbprint).expect("消す");
        assert_eq!(list.certificates().expect("空"), []);
        assert_eq!(
            list.trust(&summary.thumbprint).expect_err("無い").kind(),
            io::ErrorKind::NotFound
        );
    }

    #[test]
    fn add_trusted_replaces_a_rejected_copy_and_rejects_garbage() {
        let root = tempfile::tempdir().expect("試験用ディレクトリ");
        let list = TrustList::new(root.path().join("own"));
        let (der, summary) = peer(root.path(), "peer");
        let cert = X509::from_der(&der).expect("DER");
        ensure_dirs(list.pki_dir()).expect("一覧を作る");
        std::fs::write(list.file_for(&cert, TrustState::Rejected), &der).expect("拒否に置く");
        std::fs::write(list.pki_dir().join("rejected/notes.txt"), b"x").expect("無関係なファイル");

        assert_eq!(list.add_trusted(&der).expect("信頼に置く"), summary);
        let listed = list.certificates().expect("一覧");
        assert_eq!(
            listed.len(),
            1,
            "読めないファイルは飛ばし、拒否の写しは消す"
        );
        assert_eq!(listed[0].state, TrustState::Trusted);
        assert_eq!(
            list.add_trusted(b"not a certificate")
                .expect_err("壊れた DER")
                .kind(),
            io::ErrorKind::InvalidData
        );
    }

    #[test]
    fn pending_certificates_go_to_rejected_once() {
        let root = tempfile::tempdir().expect("試験用ディレクトリ");
        let list = TrustList::new(root.path().join("own"));
        let (der, summary) = peer(root.path(), "peer");
        assert_eq!(list.state_of(&summary.thumbprint).expect("無い"), None);
        assert_eq!(
            list.add_pending(&der).expect("保留にする"),
            (summary.clone(), TrustState::Rejected)
        );
        list.trust(&summary.thumbprint).expect("信頼する");
        assert_eq!(
            list.add_pending(&der).expect("信頼済みは変えない"),
            (summary.clone(), TrustState::Trusted)
        );
        assert_eq!(list.certificates().expect("一覧").len(), 1);
    }
}
