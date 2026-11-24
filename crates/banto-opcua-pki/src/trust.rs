//! 相手の証明書の信頼の一覧（ADR-0009）。

use std::{
    io,
    path::{Path, PathBuf},
};

use banto_opcua_core::CertificateInfo;
use opcua::crypto::X509;

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
/// 証明書は拇印（40 桁の 16 進。大文字・小文字は問わない）で指す。
///
/// ファイル名は拇印だけから作る（`<小文字の拇印>.der`）。相手が決める CN をパスに
/// 入れないので、保存先は必ず `trusted/` か `rejected/` の直下になる。両方にある
/// 証明書は、async-opcua と同じく拒否として扱う。移す途中で止まっても、信頼の側に
/// 倒れない順で書く。正規の名前でないファイル（旧い `CN [拇印].der` の名前を含む）は
/// 一覧にも判定にも使わず、[`Self::migrate`] が正規の名前へ移す。
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
    /// 両方にある証明書は拒否として 1 回だけ載せる。証明書として読めないファイルは飛ばす。
    pub fn certificates(&self) -> io::Result<Vec<TrustedCertificate>> {
        self.migrate()?;
        let rejected = self.read_dir(TrustState::Rejected)?;
        let mut trusted: Vec<_> = self
            .read_dir(TrustState::Trusted)?
            .into_iter()
            .filter(|(name, _)| !rejected.iter().any(|(other, _)| other == name))
            .collect();
        let mut all = Vec::new();
        for (state, listed) in [
            (TrustState::Trusted, std::mem::take(&mut trusted)),
            (TrustState::Rejected, rejected),
        ] {
            let mut listed: Vec<_> = listed
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
    /// どちらにも無ければ `NotFound`。信頼の側へ書いてから拒否の側を消すので、
    /// 途中で止まれば拒否のまま残る。
    pub fn trust(&self, thumbprint: &str) -> io::Result<CertificateInfo> {
        let (cert, _) = self.locate(thumbprint)?;
        self.place(&cert, TrustState::Trusted)?;
        self.discard(&cert, TrustState::Rejected)?;
        Ok(info::summarize(&cert))
    }

    /// 信頼の一覧にある証明書を、拒否の一覧へ移す。既に拒否していれば何もしない。
    /// どちらにも無ければ `NotFound`。拒否の側へ書いてから信頼の側を消す。
    pub fn reject(&self, thumbprint: &str) -> io::Result<CertificateInfo> {
        let (cert, _) = self.locate(thumbprint)?;
        self.place(&cert, TrustState::Rejected)?;
        self.discard(&cert, TrustState::Trusted)?;
        Ok(info::summarize(&cert))
    }

    /// 両方の一覧から消す。次にその相手が来たら、初めての証明書として拒否の一覧に置かれる。
    /// どちらにも無ければ `NotFound`。信頼の側を先に消す。
    pub fn remove(&self, thumbprint: &str) -> io::Result<()> {
        let (cert, _) = self.locate(thumbprint)?;
        self.discard(&cert, TrustState::Trusted)?;
        self.discard(&cert, TrustState::Rejected)
    }

    /// DER の証明書を信頼の一覧へ置く（相手の証明書を前もって受け取ったとき）。
    /// 拒否の一覧に同じ証明書があれば消す。証明書として読めなければ `InvalidData`。
    pub fn add_trusted(&self, der: &[u8]) -> io::Result<CertificateInfo> {
        let cert = X509::from_der(der).map_err(|_| info::invalid_certificate())?;
        self.migrate()?;
        self.place(&cert, TrustState::Trusted)?;
        self.discard(&cert, TrustState::Rejected)?;
        Ok(info::summarize(&cert))
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
        self.place(&cert, TrustState::Rejected)?;
        Ok((summary, TrustState::Rejected))
    }

    /// 拇印の証明書がどちらの一覧にあるか。両方にあれば拒否を返す（async-opcua の
    /// 接続時の判定と同じ）。どちらにも無ければ `None`。
    pub fn state_of(&self, thumbprint: &str) -> io::Result<Option<TrustState>> {
        self.migrate()?;
        let name = file_name_for_thumbprint(thumbprint)?;
        for state in [TrustState::Rejected, TrustState::Trusted] {
            if self.read_named(state, &name)?.is_some() {
                return Ok(Some(state));
            }
        }
        Ok(None)
    }

    /// 正規の名前でない証明書のファイルを、正規の名前（`<小文字の拇印>.der`）へ移す。
    ///
    /// 旧い async-opcua が `CN [拇印].der` の名前で書いた一覧を引き継ぐためのもの。
    /// 同じ証明書が正規の名前で既にあれば、旧い方を消すだけにする。証明書として
    /// 読めないファイルには触らない。移した数を返す。一覧のほかの操作も先にこれを呼ぶ。
    pub fn migrate(&self) -> io::Result<usize> {
        let mut moved = 0;
        for state in [TrustState::Trusted, TrustState::Rejected] {
            let dir = self.dir.join(dir_name(state));
            for (path, name) in list_files(&dir)? {
                if is_canonical_name(&name) {
                    continue;
                }
                let Some(cert) = read_certificate(&path)? else {
                    continue;
                };
                let target = dir.join(file_name(&cert));
                if !target.try_exists()? {
                    std::fs::write(&target, der_of(&cert)?)?;
                }
                std::fs::remove_file(&path)?;
                tracing::info!(
                    from = %path.display(),
                    to = %target.display(),
                    "証明書のファイルを拇印の名前へ移しました"
                );
                moved += 1;
            }
        }
        Ok(moved)
    }

    /// 拇印の証明書を、拒否・信頼の順に探す。
    fn locate(&self, thumbprint: &str) -> io::Result<(X509, TrustState)> {
        self.migrate()?;
        let name = file_name_for_thumbprint(thumbprint)?;
        for state in [TrustState::Rejected, TrustState::Trusted] {
            if let Some(cert) = self.read_named(state, &name)? {
                return Ok((cert, state));
            }
        }
        Err(not_found(thumbprint))
    }

    fn place(&self, cert: &X509, state: TrustState) -> io::Result<()> {
        ensure_dirs(&self.dir)?;
        let path = self.dir.join(dir_name(state)).join(file_name(cert));
        if path.try_exists()? {
            return Ok(());
        }
        std::fs::write(path, der_of(cert)?)
    }

    fn discard(&self, cert: &X509, state: TrustState) -> io::Result<()> {
        let path = self.dir.join(dir_name(state)).join(file_name(cert));
        match std::fs::remove_file(path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            other => other,
        }
    }

    /// 正規の名前の 1 枚。名前と中身の拇印が合わなければ無いものとして扱う
    /// （async-opcua もその信頼を使わない）。
    fn read_named(&self, state: TrustState, name: &str) -> io::Result<Option<X509>> {
        let path = self.dir.join(dir_name(state)).join(name);
        if !path.try_exists()? {
            return Ok(None);
        }
        Ok(read_certificate(&path)?.filter(|cert| file_name(cert) == name))
    }

    /// 正規の名前で、中身の拇印が名前と合う証明書だけを返す。
    fn read_dir(&self, state: TrustState) -> io::Result<Vec<(String, X509)>> {
        let mut certs = Vec::new();
        for (path, name) in list_files(&self.dir.join(dir_name(state)))? {
            if !is_canonical_name(&name) {
                continue;
            }
            if let Some(cert) = read_certificate(&path)?.filter(|cert| file_name(cert) == name) {
                certs.push((name, cert));
            }
        }
        Ok(certs)
    }
}

/// 一覧での証明書のファイル名。拇印の 16 進だけから作る。async-opcua の
/// `CertificateStore::cert_file_name` と同じ名前になる。
fn file_name(cert: &X509) -> String {
    format!("{}.der", info::thumbprint(cert).to_ascii_lowercase())
}

/// 利用者が渡した拇印から、ファイル名を作る。40 桁の 16 進でなければ `InvalidInput`。
fn file_name_for_thumbprint(thumbprint: &str) -> io::Result<String> {
    let thumbprint = thumbprint.trim();
    if thumbprint.len() == 40 && thumbprint.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(format!("{}.der", thumbprint.to_ascii_lowercase()))
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("拇印は 40 桁の 16 進で指定してください: {thumbprint}"),
        ))
    }
}

fn is_canonical_name(name: &str) -> bool {
    name.strip_suffix(".der").is_some_and(|stem| {
        stem.len() == 40
            && stem
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    })
}

/// ディレクトリ直下の通常のファイルと、その名前。ディレクトリが無ければ空。
/// 名前が UTF-8 でないファイルは飛ばす（正規の名前ではない）。
fn list_files(dir: &Path) -> io::Result<Vec<(PathBuf, String)>> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let mut files = Vec::new();
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        if let Ok(name) = entry.file_name().into_string() {
            files.push((entry.path(), name));
        }
    }
    Ok(files)
}

/// 証明書のファイルを読む。読めなければ（I/O の失敗）`Err`、証明書として解釈できなければ `None`。
fn read_certificate(path: &Path) -> io::Result<Option<X509>> {
    let bytes = std::fs::read(path)?;
    let cert = X509::from_der(&bytes).or_else(|_| X509::from_pem(&bytes));
    if cert.is_err() {
        tracing::debug!(path = %path.display(), "証明書として読めないファイルを飛ばします");
    }
    Ok(cert.ok())
}

fn der_of(cert: &X509) -> io::Result<Vec<u8>> {
    cert.to_der().map_err(|_| info::invalid_certificate())
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

    fn stored(list: &TrustList, cert: &X509, state: TrustState) -> PathBuf {
        list.pki_dir().join(dir_name(state)).join(file_name(cert))
    }

    #[test]
    fn file_names_match_async_opcua_and_come_from_the_thumbprint_only() {
        let root = tempfile::tempdir().expect("試験用ディレクトリ");
        let (der, summary) = peer(root.path(), "peer");
        let cert = X509::from_der(&der).expect("DER");
        assert_eq!(
            file_name(&cert),
            opcua::crypto::CertificateStore::cert_file_name(&cert),
            "async-opcua が探す名前と同じ"
        );
        assert_eq!(
            file_name(&cert),
            format!("{}.der", summary.thumbprint.to_ascii_lowercase())
        );
        assert!(is_canonical_name(&file_name(&cert)));
        let list = TrustList::new(root.path().join("own"));
        for bad in [
            "../../etc/passwd",
            "",
            "AB",
            &format!("{}/x", summary.thumbprint),
        ] {
            assert_eq!(
                list.trust(bad).expect_err("拇印でない").kind(),
                io::ErrorKind::InvalidInput,
                "{bad}"
            );
        }
    }

    #[test]
    fn a_certificate_in_both_lists_is_rejected() {
        let root = tempfile::tempdir().expect("試験用ディレクトリ");
        let list = TrustList::new(root.path().join("own"));
        let (der, summary) = peer(root.path(), "peer");
        let cert = X509::from_der(&der).expect("DER");
        ensure_dirs(list.pki_dir()).expect("一覧を作る");
        for state in [TrustState::Trusted, TrustState::Rejected] {
            std::fs::write(stored(&list, &cert, state), &der).expect("両方に置く");
        }
        assert_eq!(
            list.state_of(&summary.thumbprint).expect("状態"),
            Some(TrustState::Rejected)
        );
        let listed = list.certificates().expect("一覧");
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].state, TrustState::Rejected);
    }

    #[test]
    fn misnamed_files_are_never_trusted() {
        let root = tempfile::tempdir().expect("試験用ディレクトリ");
        let list = TrustList::new(root.path().join("own"));
        let (der, summary) = peer(root.path(), "peer");
        let (other, _) = peer(root.path(), "other");
        ensure_dirs(list.pki_dir()).expect("一覧を作る");
        // 正規の名前だが中身は別の証明書。async-opcua もこの信頼は使わない。
        let name = format!("{}.der", summary.thumbprint.to_ascii_lowercase());
        std::fs::write(list.pki_dir().join("trusted").join(&name), &other).expect("置く");
        assert_eq!(list.state_of(&summary.thumbprint).expect("状態"), None);
        assert_eq!(
            list.add_pending(&der).expect("保留").1,
            TrustState::Rejected
        );
    }

    #[test]
    fn legacy_names_are_migrated_in_place() {
        let root = tempfile::tempdir().expect("試験用ディレクトリ");
        let list = TrustList::new(root.path().join("own"));
        let (der, summary) = peer(root.path(), "peer");
        let (other, other_summary) = peer(root.path(), "other");
        ensure_dirs(list.pki_dir()).expect("一覧を作る");
        let legacy = |name: &str| {
            format!(
                "CN={name} [{}].der",
                if name == "peer" {
                    summary.thumbprint.to_ascii_lowercase()
                } else {
                    other_summary.thumbprint.to_ascii_lowercase()
                }
            )
        };
        let trusted = list.pki_dir().join("trusted");
        std::fs::write(trusted.join(legacy("peer")), &der).expect("旧い名前で信頼");
        std::fs::write(
            list.pki_dir().join("rejected").join(legacy("other")),
            &other,
        )
        .expect("旧い名前で拒否");
        // 同じ証明書が正規の名前でもある。旧い方は消すだけ。
        let cert = X509::from_der(&der).expect("DER");
        std::fs::write(trusted.join("CN=dup [x].der"), &der).expect("重複");
        std::fs::write(stored(&list, &cert, TrustState::Trusted), &der).expect("正規");
        std::fs::write(trusted.join("README.txt"), b"keep").expect("無関係なファイル");

        assert_eq!(list.migrate().expect("移す"), 3);
        assert_eq!(
            list.state_of(&summary.thumbprint).expect("状態"),
            Some(TrustState::Trusted)
        );
        assert_eq!(
            list.state_of(&other_summary.thumbprint).expect("状態"),
            Some(TrustState::Rejected)
        );
        let names: Vec<_> = std::fs::read_dir(&trusted)
            .expect("読む")
            .map(|entry| {
                entry
                    .expect("項目")
                    .file_name()
                    .into_string()
                    .expect("名前")
            })
            .collect();
        let names: std::collections::BTreeSet<_> = names.into_iter().collect();
        assert_eq!(
            names,
            ["README.txt".to_owned(), file_name(&cert)]
                .into_iter()
                .collect()
        );
        assert_eq!(list.migrate().expect("2 回目"), 0);
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
        std::fs::write(stored(&list, &cert, TrustState::Rejected), &der).expect("拒否に置く");
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
        assert!(stored(&list, &cert, TrustState::Trusted).is_file());
        assert!(!stored(&list, &cert, TrustState::Rejected).exists());
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
        std::fs::write(stored(&list, &cert, TrustState::Rejected), &der).expect("拒否に置く");
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
