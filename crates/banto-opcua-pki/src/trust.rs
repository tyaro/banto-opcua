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

    /// 旧い async-opcua が書いた名前の証明書のファイルを、正規の名前（`<小文字の拇印>.der`）へ移す。
    ///
    /// 移すのは、旧い async-opcua が実際に照合していた名前（`<CN> [<小文字の拇印>].der`。
    /// CN は `CN=` 付きで前後の空白と `/` を除いたもの）と、中身の証明書から作った
    /// その名前が一致するファイルだけ。それ以外の名前のファイル（保管用に置いた
    /// 証明書など）は旧い版でも信頼に使われていなかったので、触らず、信頼にも数えない。
    /// 管理者が [`Self::add_trusted`] などで明示したときだけ一覧に入る。
    ///
    /// 名前が旧い形（`<空でない前置き> [<40 桁の小文字の 16 進>].der`）に合わないファイルは
    /// 開きもしない。置き場をほかのアプリが守るために置いた、利用者には読めない印の
    /// ファイルなどがあっても、それを理由に失敗しない。
    ///
    /// 移し先は一時ファイルに書いて名前を変えて確定し、読み戻して同じ証明書であることを
    /// 確かめてから移し元を消す。移し先に正しい証明書が既にあれば、移し元を消すだけに
    /// する。途中で失敗したら移し元は残る。移した数を返す。一覧のほかの操作も先にこれを呼ぶ。
    pub fn migrate(&self) -> io::Result<usize> {
        let mut moved = 0;
        for state in [TrustState::Trusted, TrustState::Rejected] {
            let dir = self.dir.join(dir_name(state));
            for (path, name) in list_files(&dir)? {
                // 名前の形だけで先に絞り、旧い名前になりえないファイルは開かない。
                if !is_legacy_shaped_name(&name) {
                    continue;
                }
                let Some(cert) = read_certificate(&path)? else {
                    continue;
                };
                if legacy_file_name(&cert).as_deref() != Some(name.as_str()) {
                    continue;
                }
                let target = dir.join(file_name(&cert));
                // 大文字・小文字を区別しないファイルシステムで、同じファイルを指していないか。
                if same_file(&path, &target)? {
                    continue;
                }
                write_verified(&target, &der_of(&cert)?)?;
                match retry_shared(|| std::fs::remove_file(&path)) {
                    // 並行する移行が先に消した。移し先は上で確かめてある。
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    other => other?,
                }
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
        write_verified(&path, &der_of(cert)?)
    }

    fn discard(&self, cert: &X509, state: TrustState) -> io::Result<()> {
        let path = self.dir.join(dir_name(state)).join(file_name(cert));
        match retry_shared(|| std::fs::remove_file(&path)) {
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

/// 旧い async-opcua（`CertificateStore::cert_file_name` を拇印だけにする前）が付けた名前。
/// CN を読めなければ `None`。
fn legacy_file_name(cert: &X509) -> Option<String> {
    let prefix = cert.common_name().ok()?.trim().replace('/', "");
    let thumbprint = info::thumbprint(cert).to_ascii_lowercase();
    Some(if prefix.is_empty() {
        format!("{thumbprint}.der")
    } else {
        format!("{prefix} [{thumbprint}].der")
    })
}

/// `path` に `der` と同じ中身がある状態にする。既に同じ中身なら何もしない。
///
/// 違う中身（途中で切れた書き込みなど）なら、この呼び出しだけの一時ファイルを同じ
/// ディレクトリに排他的に作って書き、名前を変えて置き換え、読み戻して確かめる。
/// 同じ証明書を並行して置く操作があっても、互いの一時ファイルには触らない。名前の
/// 変更に失敗しても、`path` が既に正しい中身なら（ほかの操作が先に置いた）成功とする。
/// 失敗したときは自分の一時ファイルだけを消す。
fn write_verified(path: &Path, der: &[u8]) -> io::Result<()> {
    if holds(path, der)? {
        return Ok(());
    }
    let (temporary, mut file) = create_temporary(path)?;
    let written = {
        use std::io::Write;

        file.write_all(der).and_then(|()| file.sync_all())
    };
    drop(file);
    if let Err(error) = written.and_then(|()| retry_shared(|| std::fs::rename(&temporary, path))) {
        let _ = std::fs::remove_file(&temporary);
        return if holds(path, der)? {
            Ok(())
        } else {
            Err(error)
        };
    }
    if holds(path, der)? {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("書いた証明書を読み戻せません: {}", path.display()),
        ))
    }
}

/// Windows で、ほかの操作（この一覧の並行する操作や、接続を確かめる async-opcua）が
/// 開いている・消している途中のファイルへの読み書き・名前の変更・削除は、一時的に
/// `PermissionDenied`（共有違反）になる。そのときだけ、短い間を置いて試し直す。
/// Unix の `PermissionDenied` は本当の権限の問題なので、試し直さずにそのまま返す。
fn retry_shared<T>(mut operation: impl FnMut() -> io::Result<T>) -> io::Result<T> {
    const ATTEMPTS: u32 = if cfg!(windows) { 50 } else { 1 };
    let mut attempt = 1;
    loop {
        match operation() {
            Err(error) if error.kind() == io::ErrorKind::PermissionDenied && attempt < ATTEMPTS => {
                attempt += 1;
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            other => return other,
        }
    }
}

/// `path` の中身が `der` と同じか。無ければ `false`。
fn holds(path: &Path, der: &[u8]) -> io::Result<bool> {
    match retry_shared(|| std::fs::read(path)) {
        Ok(existing) => Ok(existing == der),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error),
    }
}

/// `path` の隣に、この呼び出しだけの一時ファイルを `create_new` で作る。名前は
/// プロセス ID と連番で分け、既にあれば次の番号を試す。
fn create_temporary(path: &Path) -> io::Result<(PathBuf, std::fs::File)> {
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT: AtomicU64 = AtomicU64::new(0);
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "ファイル名がありません"))?;
    loop {
        let temporary = path.with_file_name(format!(
            ".{file_name}.{}-{}.tmp",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
        {
            Ok(file) => return Ok((temporary, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
}

/// 2 つのパスが同じファイルを指すか。片方が無ければ違うとする。
fn same_file(a: &Path, b: &Path) -> io::Result<bool> {
    let canonical = |path: &Path| match retry_shared(|| std::fs::canonicalize(path)) {
        Ok(path) => Ok(Some(path)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    };
    Ok(match (canonical(a)?, canonical(b)?) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    })
}

fn is_canonical_name(name: &str) -> bool {
    name.strip_suffix(".der")
        .is_some_and(is_lowercase_thumbprint)
}

/// [`legacy_file_name`] が CN のある証明書に付ける名前の形か
/// （`<空でない前置き> [<小文字の拇印>].der`）。中身は見ない。
fn is_legacy_shaped_name(name: &str) -> bool {
    let Some(rest) = name.strip_suffix("].der") else {
        return false;
    };
    // CN は UTF-8 なので、文字の境目でないところでは切らない（`get` が `None` になる）。
    let at = rest.len().saturating_sub(40);
    match (rest.get(..at), rest.get(at..)) {
        (Some(head), Some(thumbprint)) => {
            is_lowercase_thumbprint(thumbprint)
                && head
                    .strip_suffix(" [")
                    .is_some_and(|prefix| !prefix.is_empty())
        }
        _ => false,
    }
}

fn is_lowercase_thumbprint(text: &str) -> bool {
    text.len() == 40
        && text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
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

/// 証明書のファイルを読む。読めなければ（I/O の失敗）`Err`、無いか証明書として解釈できなければ `None`。
fn read_certificate(path: &Path) -> io::Result<Option<X509>> {
    let bytes = match retry_shared(|| std::fs::read(path)) {
        Ok(bytes) => bytes,
        // 一覧を読んでから開くまでに、並行する操作が消した。
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
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

    /// 旧い async-opcua が付けた名前。
    fn legacy(cert_der: &[u8]) -> String {
        legacy_file_name(&X509::from_der(cert_der).expect("DER")).expect("CN がある")
    }

    #[test]
    fn legacy_names_match_the_old_async_opcua_format() {
        let root = tempfile::tempdir().expect("試験用ディレクトリ");
        let (der, summary) = peer(root.path(), "peer");
        assert_eq!(
            legacy(&der),
            format!("CN=peer [{}].der", summary.thumbprint.to_ascii_lowercase())
        );
    }

    #[test]
    fn only_names_the_old_version_trusted_are_migrated() {
        let root = tempfile::tempdir().expect("試験用ディレクトリ");
        let list = TrustList::new(root.path().join("own"));
        let (der, summary) = peer(root.path(), "peer");
        let (other, other_summary) = peer(root.path(), "other");
        let (spare, spare_summary) = peer(root.path(), "spare");
        ensure_dirs(list.pki_dir()).expect("一覧を作る");
        let trusted = list.pki_dir().join("trusted");
        std::fs::write(trusted.join(legacy(&der)), &der).expect("旧い名前で信頼");
        std::fs::write(list.pki_dir().join("rejected").join(legacy(&other)), &other)
            .expect("旧い名前で拒否");
        // 旧い版も照合しなかった名前。保管用に置いただけの証明書は信頼に変えない。
        std::fs::write(trusted.join("backup.der"), &spare).expect("任意の名前");
        std::fs::write(
            trusted.join(legacy(&der).replace("CN=peer", "CN=renamed")),
            &spare,
        )
        .expect("中身と合わない旧い形の名前");
        std::fs::write(trusted.join("README.txt"), b"keep").expect("無関係なファイル");

        assert_eq!(list.migrate().expect("移す"), 2);
        assert_eq!(
            list.state_of(&summary.thumbprint).expect("状態"),
            Some(TrustState::Trusted)
        );
        assert_eq!(
            list.state_of(&other_summary.thumbprint).expect("状態"),
            Some(TrustState::Rejected)
        );
        assert_eq!(
            list.state_of(&spare_summary.thumbprint).expect("状態"),
            None,
            "任意の名前の証明書は承認なしで信頼しない"
        );
        let names: std::collections::BTreeSet<_> = std::fs::read_dir(&trusted)
            .expect("読む")
            .map(|entry| {
                entry
                    .expect("項目")
                    .file_name()
                    .into_string()
                    .expect("名前")
            })
            .collect();
        let cert = X509::from_der(&der).expect("DER");
        assert!(names.contains(&file_name(&cert)));
        assert!(!names.contains(&legacy(&der)), "移し元は消える");
        assert!(names.contains("backup.der") && names.contains("README.txt"));
        assert_eq!(list.migrate().expect("2 回目"), 0);
    }

    #[test]
    fn only_legacy_shaped_names_are_opened_by_migration() {
        let root = tempfile::tempdir().expect("試験用ディレクトリ");
        let (der, summary) = peer(root.path(), "peer");
        let lower = summary.thumbprint.to_ascii_lowercase();
        assert!(is_legacy_shaped_name(&legacy(&der)));
        assert!(is_legacy_shaped_name(&format!("CN=計測 [{lower}].der")));
        for name in [
            format!("{lower}.der"),
            format!(" [{lower}].der"),
            format!("CN=peer [{}].der", summary.thumbprint.to_ascii_uppercase()),
            format!("CN=peer [{lower}].pem"),
            ".banto-safe-root".to_owned(),
            "backup.der".to_owned(),
            "計測].der".to_owned(),
            "].der".to_owned(),
        ] {
            assert!(!is_legacy_shaped_name(&name), "{name}");
        }
    }

    /// `path` を、この試験のプロセスから読めなくする。読めなくできたら `true`
    /// （root など権限を無視して読める環境では `false`）。
    fn make_unreadable(path: &Path) -> bool {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o000))
                .expect("読めなくする");
        }
        #[cfg(windows)]
        {
            // Everyone の読み出しを DACL で拒否する。拒否は Administrators にも効く。
            let status = std::process::Command::new("icacls")
                .arg(path)
                .args(["/deny", "*S-1-1-0:(R)"])
                .status()
                .expect("icacls を動かす");
            assert!(status.success(), "icacls: {status}");
        }
        std::fs::read(path).is_err()
    }

    fn make_readable(path: &Path) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o644))
                .expect("権限を戻す");
        }
        #[cfg(windows)]
        {
            let status = std::process::Command::new("icacls")
                .arg(path)
                .args(["/remove:d", "*S-1-1-0"])
                .status()
                .expect("icacls を動かす");
            assert!(status.success(), "icacls: {status}");
        }
    }

    /// 置き場を守るほかのアプリが `trusted/`・`rejected/` に置いた、読めない印の
    /// ファイルがあっても、移行と自分の証明書の用意は成功する（Issue #64）。
    #[test]
    fn unreadable_marker_files_do_not_break_migration() {
        let root = tempfile::tempdir().expect("試験用ディレクトリ");
        let pki = root.path().join("own");
        let list = TrustList::new(&pki);
        let (der, summary) = peer(root.path(), "peer");
        ensure_dirs(&pki).expect("一覧を作る");
        std::fs::write(pki.join("trusted").join(legacy(&der)), &der).expect("旧い名前で信頼");
        let markers: Vec<_> = [TRUSTED_DIR, REJECTED_DIR]
            .into_iter()
            .map(|dir| pki.join(dir).join(".banto-safe-root"))
            .collect();
        let mut unreadable = true;
        for marker in &markers {
            std::fs::write(marker, b"marker").expect("印を置く");
            unreadable &= make_unreadable(marker);
        }
        if !unreadable {
            eprintln!("root では読めないファイルを作れないため、読めないことは確かめない");
        }

        let migrated = list.migrate();
        let prepared =
            prepare_own_certificate(&pki, &ApplicationIdentity::new("own", "urn:banto:test:own"));
        let listed = list.certificates();
        let state = list.state_of(&summary.thumbprint);
        for marker in &markers {
            make_readable(marker);
        }
        assert_eq!(migrated.expect("印は開かずに移す"), 1);
        prepared.expect("自分の証明書を用意できる");
        assert_eq!(listed.expect("一覧").len(), 1);
        assert_eq!(state.expect("状態"), Some(TrustState::Trusted));
        for marker in &markers {
            assert_eq!(std::fs::read(marker).expect("印は残る"), b"marker");
        }
    }

    /// 移し先に壊れたファイルが残っていても、正しく置き直してから移し元を消す。
    #[test]
    fn migration_repairs_a_broken_target_before_removing_the_source() {
        let root = tempfile::tempdir().expect("試験用ディレクトリ");
        let list = TrustList::new(root.path().join("own"));
        let (der, summary) = peer(root.path(), "peer");
        let cert = X509::from_der(&der).expect("DER");
        ensure_dirs(list.pki_dir()).expect("一覧を作る");
        let trusted = list.pki_dir().join("trusted");
        std::fs::write(trusted.join(legacy(&der)), &der).expect("旧い名前で信頼");
        let target = stored(&list, &cert, TrustState::Trusted);
        std::fs::write(&target, &der[..der.len() / 2]).expect("途中で切れた移し先");

        assert_eq!(list.migrate().expect("移す"), 1);
        assert_eq!(std::fs::read(&target).expect("移し先"), der);
        assert!(!trusted.join(legacy(&der)).exists());
        assert_eq!(
            list.state_of(&summary.thumbprint).expect("状態"),
            Some(TrustState::Trusted)
        );
    }

    /// 壊れた既存のファイルを、置き済みとして扱わない。
    #[test]
    fn placing_over_a_broken_file_rewrites_it() {
        let root = tempfile::tempdir().expect("試験用ディレクトリ");
        let list = TrustList::new(root.path().join("own"));
        let (der, summary) = peer(root.path(), "peer");
        let cert = X509::from_der(&der).expect("DER");
        ensure_dirs(list.pki_dir()).expect("一覧を作る");
        let path = stored(&list, &cert, TrustState::Trusted);
        std::fs::write(&path, b"broken").expect("壊れたファイル");
        assert_eq!(list.state_of(&summary.thumbprint).expect("状態"), None);
        list.add_trusted(&der).expect("置き直す");
        assert_eq!(std::fs::read(&path).expect("読む"), der);
        assert_eq!(
            list.state_of(&summary.thumbprint).expect("状態"),
            Some(TrustState::Trusted)
        );
    }

    /// 同じ一覧で、同じ証明書の移行と信頼を並行して行っても、壊れず信頼に落ち着き、
    /// 一時ファイルも残らない。
    #[test]
    fn concurrent_operations_on_one_certificate_settle() {
        let root = tempfile::tempdir().expect("試験用ディレクトリ");
        let list = TrustList::new(root.path().join("own"));
        let (der, summary) = peer(root.path(), "peer");
        let cert = X509::from_der(&der).expect("DER");
        for round in 0..20 {
            ensure_dirs(list.pki_dir()).expect("一覧を作る");
            let trusted = list.pki_dir().join("trusted");
            std::fs::write(trusted.join(legacy(&der)), &der).expect("旧い名前で信頼");
            std::thread::scope(|scope| {
                let workers: Vec<_> = (0..8)
                    .map(|worker| {
                        let list = list.clone();
                        let der = der.clone();
                        scope.spawn(move || match worker % 3 {
                            0 => list.migrate().map(|_| ()),
                            1 => list.add_trusted(&der).map(|_| ()),
                            _ => list.certificates().map(|_| ()),
                        })
                    })
                    .collect();
                for worker in workers {
                    worker.join().expect("スレッド").expect("操作は成功する");
                }
            });
            assert_eq!(
                list.state_of(&summary.thumbprint).expect("状態"),
                Some(TrustState::Trusted),
                "{round} 回目"
            );
            assert_eq!(
                std::fs::read(stored(&list, &cert, TrustState::Trusted)).expect("読む"),
                der
            );
            let leftovers: Vec<_> = std::fs::read_dir(&trusted)
                .expect("読む")
                .map(|entry| entry.expect("項目").file_name())
                .filter(|name| name.to_string_lossy().ends_with(".tmp"))
                .collect();
            assert!(leftovers.is_empty(), "{leftovers:?}");
            list.remove(&summary.thumbprint)
                .expect("次の回のために消す");
        }
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
