//! 自分の鍵と自己署名の証明書（ADR-0003、ADR-0009）。

use std::{io, path::Path, time::SystemTime};

use banto_opcua_core::CertificateInfo;
use opcua::{
    crypto::{CertificateStore, KeySize, PrivateKey, X509, X509Data},
    types::{ApplicationDescription, LocalizedText},
};

use crate::{TrustList, info, trust};

/// `pki_dir` からの、自分の証明書の位置。
pub const CERTIFICATE_PATH: &str = "own/cert.der";
/// `pki_dir` からの、自分の秘密鍵の位置。
pub const PRIVATE_KEY_PATH: &str = "private/private.pem";

/// 自己署名の証明書の有効日数。期限が来ると次の起動で作り直し、拇印が変わる
/// （相手は信頼し直す）。年に 1 回の作業にしないよう 5 年にする（ADR-0009）。
const CERTIFICATE_DURATION_DAYS: u32 = 5 * 365 + 1;

/// 証明書に載せる、自分のアプリケーションの名乗り。
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ApplicationIdentity {
    /// ApplicationName。証明書の CN と O にもなる。
    pub application_name: String,
    /// ApplicationUri。証明書の SubjectAltName の URI と一致させる（Part 4）。
    pub application_uri: String,
    /// SubjectAltName に必ず載せるホスト名か IP アドレス（IPv6 は括弧なし）。
    ///
    /// `localhost`・`127.0.0.1`・`::1`・この機械のホスト名とその IP アドレスは
    /// 常に載る。相手がこれ以外の名前で接続するなら（サーバーの公開ホストなど）ここに足す。
    pub hosts: Vec<String>,
}

impl ApplicationIdentity {
    pub fn new(application_name: impl Into<String>, application_uri: impl Into<String>) -> Self {
        Self {
            application_name: application_name.into(),
            application_uri: application_uri.into(),
            hosts: Vec::new(),
        }
    }

    /// SubjectAltName に載せるホストを足す。前後の空白、IPv6 の括弧、DNS 名の末尾の
    /// ドットは除く。空なら足さない。
    pub fn with_host(mut self, host: &str) -> Self {
        let host = host.trim();
        let host = host
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'))
            .unwrap_or(host);
        let host = host.strip_suffix('.').unwrap_or(host);
        if !host.is_empty() && !self.hosts.iter().any(|known| known == host) {
            self.hosts.push(host.to_owned());
        }
        self
    }
}

/// `dir` に自分の鍵と自己署名の証明書を用意し、証明書の要約を返す。
///
/// - `dir` と `trusted/`・`rejected/` が無ければ作る。Unix では秘密鍵の親を
///   生成の前に 0700 にし（既存なら絞り）、生成後に秘密鍵を 0600 にする。
/// - 証明書と秘密鍵の片方だけが残っていたら消す（初回生成の途中で落ちたなど）。
/// - 両方あれば読み直す。中身を証明書・秘密鍵として解釈できない、秘密鍵が証明書の
///   公開鍵と組にならない、ApplicationUri が違う、`identity.hosts` のどれかが
///   SubjectAltName に無い、有効期間の外、のどれかなら、警告を出して両方を消し、
///   作り直す（拇印が変わるので、相手は信頼し直す）。
/// - 無ければ 2048 ビットの RSA 鍵と、有効期間 5 年の証明書を作る。
/// - 旧い名前の信頼の一覧を、拇印だけの名前へ移す（[`crate::TrustList::migrate`]）。
///
/// 存在を確かめられない、ファイルを読めない（権限エラー、一時的な I/O の失敗など）
/// ときは、何も消さず何も作らずに `Err` を返す。中身が壊れているのと区別し、
/// 一時的な障害で拇印を変えない。
/// 同じディレクトリを、同時に起動する複数のプロセスで共有しない（生成が競合する）。
pub fn prepare_own_certificate(
    dir: &Path,
    identity: &ApplicationIdentity,
) -> io::Result<CertificateInfo> {
    std::fs::create_dir_all(dir)?;
    protect_private_dir(&dir.join(PRIVATE_KEY_PATH))?;
    trust::ensure_dirs(dir)?;
    TrustList::new(dir).migrate()?;
    discard_unpaired_keypair(dir)?;

    let cert_path = dir.join(CERTIFICATE_PATH);
    let key_path = dir.join(PRIVATE_KEY_PATH);
    if cert_path.try_exists()? {
        // 読めないのは I/O の障害で、`?` で止める。中身の不一致だけを作り直しにする。
        let cert_bytes = std::fs::read(&cert_path)?;
        let key_bytes = std::fs::read(&key_path)?;
        match check_existing(&cert_bytes, &key_bytes, identity) {
            Ok(summary) => {
                restrict_private_key(&key_path)?;
                return Ok(summary);
            }
            Err(reason) => {
                tracing::warn!(
                    path = %cert_path.display(),
                    %reason,
                    "自分の証明書が設定と合わないため、鍵と証明書を作り直します。相手は新しい証明書を信頼し直す必要があります"
                );
                std::fs::remove_file(&cert_path)?;
                std::fs::remove_file(&key_path)?;
            }
        }
    }

    let description = ApplicationDescription {
        application_uri: identity.application_uri.as_str().into(),
        application_name: LocalizedText::new("", &identity.application_name),
        ..Default::default()
    };
    let mut data = X509Data::from((description, Some(identity.hosts.clone())));
    data.certificate_duration_days = CERTIFICATE_DURATION_DAYS;
    let (cert, _) = CertificateStore::create_certificate_and_key(
        &data, false, &cert_path, &key_path,
    )
    .map_err(|reason| io::Error::other(format!("証明書と秘密鍵を作れませんでした: {reason}")))?;
    restrict_private_key(&key_path)?;
    Ok(info::summarize(&cert))
}

/// 読み込んだ組を使えるか。使えなければ理由を返す。
fn check_existing(
    cert_bytes: &[u8],
    key_bytes: &[u8],
    identity: &ApplicationIdentity,
) -> Result<CertificateInfo, String> {
    let cert = X509::from_der(cert_bytes)
        .or_else(|_| X509::from_pem(cert_bytes))
        .map_err(|_| "証明書を解釈できません".to_owned())?;
    let key = PrivateKey::from_pem(key_bytes).map_err(|_| "秘密鍵を解釈できません".to_owned())?;
    if !key_matches(&cert, &key) {
        return Err("秘密鍵が証明書の公開鍵と組になりません".to_owned());
    }
    cert.is_application_uri_valid(&identity.application_uri)
        .map_err(|_| {
            format!(
                "ApplicationUri {} が載っていません",
                identity.application_uri
            )
        })?;
    for host in &identity.hosts {
        cert.is_hostname_valid(host)
            .map_err(|_| format!("ホスト {host} が載っていません"))?;
    }
    let summary = info::summarize(&cert);
    if !summary.is_valid_at(SystemTime::now()) {
        return Err("有効期間の外です".to_owned());
    }
    Ok(summary)
}

/// 秘密鍵で署名したものを、証明書の公開鍵で検証できるか。
fn key_matches(cert: &X509, key: &PrivateKey) -> bool {
    const PROBE: &[u8] = b"banto-opcua key pair check";
    let Ok(public) = cert.public_key() else {
        return false;
    };
    let mut signature = vec![0; key.size()];
    key.sign_sha256(PROBE, &mut signature).is_ok()
        && public.verify_sha256(PROBE, &signature).unwrap_or(false)
}

/// 証明書と秘密鍵の片方だけが残っていたら消す。
///
/// 生成は既存のファイルを上書きしないので、片方だけ残ると組み違いか、鍵の無いまま
/// 起動する。権限エラーなどを「無い」と取り違えて、残っている片方を消さない。
fn discard_unpaired_keypair(dir: &Path) -> io::Result<()> {
    let cert = dir.join(CERTIFICATE_PATH);
    let key = dir.join(PRIVATE_KEY_PATH);
    let lone = match (cert.try_exists()?, key.try_exists()?) {
        (true, false) => cert,
        (false, true) => key,
        _ => return Ok(()),
    };
    tracing::warn!(
        path = %lone.display(),
        "証明書と秘密鍵の片方だけが残っているため消して作り直します"
    );
    std::fs::remove_file(&lone)
}

/// 秘密鍵自体も所有者だけが読めるようにする。生成は umask の権限で書くが、
/// 親ディレクトリは [`protect_private_dir`] が生成前に 0700 にしているので、
/// ここまでの間も他のユーザーは辿れない。
#[cfg(unix)]
fn restrict_private_key(key: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    std::fs::set_permissions(key, std::fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn restrict_private_key(_key: &Path) -> io::Result<()> {
    Ok(())
}

/// 秘密鍵のディレクトリを、鍵を生成・読み込みする前に所有者だけのものにする。
/// 無ければ 0700 で作り、既にあれば権限を 0700 に絞る。Windows の ACL は
/// 呼び出し側のデータディレクトリに従う。
#[cfg(unix)]
fn protect_private_dir(key: &Path) -> io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

    let Some(dir) = key.parent() else {
        return Ok(());
    };
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
fn protect_private_dir(key: &Path) -> io::Result<()> {
    match key.parent() {
        Some(dir) => std::fs::create_dir_all(dir),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> ApplicationIdentity {
        ApplicationIdentity::new("banto-test", "urn:banto:test")
    }

    #[test]
    fn hosts_are_normalized_and_deduplicated() {
        let identity = identity()
            .with_host("[2001:db8::1]")
            .with_host("plc.example.com.")
            .with_host(" plc.example.com ")
            .with_host("");
        assert_eq!(identity.hosts, ["2001:db8::1", "plc.example.com"]);
    }

    /// 2 回目は同じ組を使い、名乗りやホストが変わったら作り直す。
    #[test]
    fn keypair_is_reused_until_the_identity_changes() {
        let root = tempfile::tempdir().expect("試験用ディレクトリ");
        let dir = root.path().join("pki");
        let first = prepare_own_certificate(&dir, &identity()).expect("初回に作る");
        assert!(dir.join("trusted").is_dir() && dir.join("rejected").is_dir());
        assert!(first.is_valid_at(SystemTime::now()));
        let again = prepare_own_certificate(&dir, &identity()).expect("読み直す");
        assert_eq!(again, first);

        let with_host =
            prepare_own_certificate(&dir, &identity().with_host("plc.example.com")).expect("足す");
        assert_ne!(
            with_host.thumbprint, first.thumbprint,
            "ホストが無ければ作り直す"
        );
        let renamed = prepare_own_certificate(
            &dir,
            &ApplicationIdentity::new("banto-test", "urn:banto:other").with_host("plc.example.com"),
        )
        .expect("URI を変える");
        assert_ne!(
            renamed.thumbprint, with_host.thumbprint,
            "URI が違えば作り直す"
        );
    }

    #[test]
    fn unreadable_certificate_is_regenerated() {
        let root = tempfile::tempdir().expect("試験用ディレクトリ");
        let dir = root.path();
        let first = prepare_own_certificate(dir, &identity()).expect("初回に作る");
        std::fs::write(dir.join(CERTIFICATE_PATH), b"broken").expect("壊す");
        let second = prepare_own_certificate(dir, &identity()).expect("作り直す");
        assert_ne!(second.thumbprint, first.thumbprint);
    }

    /// 別の組の秘密鍵に差し替わっていたら、使わずに作り直す。
    #[test]
    fn mismatched_private_key_is_regenerated() {
        let root = tempfile::tempdir().expect("試験用ディレクトリ");
        let dir = root.path().join("pki");
        let other = root.path().join("other");
        let first = prepare_own_certificate(&dir, &identity()).expect("初回に作る");
        prepare_own_certificate(&other, &identity()).expect("別の組を作る");
        std::fs::copy(other.join(PRIVATE_KEY_PATH), dir.join(PRIVATE_KEY_PATH))
            .expect("秘密鍵を差し替える");
        let second = prepare_own_certificate(&dir, &identity()).expect("作り直す");
        assert_ne!(second.thumbprint, first.thumbprint);
        let again = prepare_own_certificate(&dir, &identity()).expect("今度は再利用");
        assert_eq!(again, second);
    }

    /// 読めない（I/O の失敗）ときは作り直さず止め、ファイルも拇印も変えない。
    /// root でも起こせるよう、秘密鍵の場所をディレクトリにして読めなくする。
    #[test]
    fn read_failure_keeps_the_existing_pair() {
        let root = tempfile::tempdir().expect("試験用ディレクトリ");
        let dir = root.path().join("pki");
        let first = prepare_own_certificate(&dir, &identity()).expect("初回に作る");
        let cert = std::fs::read(dir.join(CERTIFICATE_PATH)).expect("証明書");
        let key_path = dir.join(PRIVATE_KEY_PATH);
        let key = std::fs::read(&key_path).expect("秘密鍵");
        let aside = root.path().join("key.pem");
        std::fs::rename(&key_path, &aside).expect("秘密鍵をよける");
        std::fs::create_dir(&key_path).expect("読めない秘密鍵");

        prepare_own_certificate(&dir, &identity()).expect_err("読めなければ止める");
        assert_eq!(
            std::fs::read(dir.join(CERTIFICATE_PATH)).expect("証明書が残る"),
            cert
        );

        std::fs::remove_dir(&key_path).expect("戻す");
        std::fs::rename(&aside, &key_path).expect("戻す");
        assert_eq!(std::fs::read(&key_path).expect("秘密鍵"), key);
        let again = prepare_own_certificate(&dir, &identity()).expect("障害が去れば再利用");
        assert_eq!(again, first);
    }

    /// 存在を確かめられないときは「無い」と扱わず、残っている鍵を消さない。
    /// root は権限を無視して読めるので、その環境では条件を作れず確かめない。
    #[cfg(unix)]
    #[test]
    fn unreadable_certificate_dir_keeps_the_keypair() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().expect("試験用ディレクトリ");
        let pki = root.path().join("pki");
        let cert = pki.join(CERTIFICATE_PATH);
        let key = pki.join(PRIVATE_KEY_PATH);
        for path in [&cert, &key] {
            std::fs::create_dir_all(path.parent().expect("親")).expect("親を作る");
            std::fs::write(path, b"keep").expect("ファイルを置く");
        }
        let own = cert.parent().expect("own");
        std::fs::set_permissions(own, std::fs::Permissions::from_mode(0o000))
            .expect("own を開けなくする");
        if cert.try_exists().is_ok() {
            std::fs::set_permissions(own, std::fs::Permissions::from_mode(0o755))
                .expect("権限を戻す");
            eprintln!("root では権限エラーを作れないため確かめない");
            return;
        }

        let result = prepare_own_certificate(&pki, &identity());
        std::fs::set_permissions(own, std::fs::Permissions::from_mode(0o755)).expect("権限を戻す");
        assert!(
            matches!(&result, Err(error) if error.kind() == io::ErrorKind::PermissionDenied),
            "確かめられないときは権限エラーにする: {result:?}"
        );
        assert_eq!(std::fs::read(&key).expect("秘密鍵が残る"), b"keep");
        assert_eq!(std::fs::read(&cert).expect("証明書が残る"), b"keep");
    }

    /// 既存の緩い秘密鍵のディレクトリも絞る。
    #[cfg(unix)]
    #[test]
    fn private_dir_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().expect("試験用ディレクトリ");
        let loose = root.path().join("loose");
        std::fs::create_dir_all(loose.join("private")).expect("既存の private");
        std::fs::set_permissions(
            loose.join("private"),
            std::fs::Permissions::from_mode(0o755),
        )
        .expect("緩い権限にする");
        protect_private_dir(&loose.join(PRIVATE_KEY_PATH)).expect("絞る");
        let mode = std::fs::metadata(loose.join("private"))
            .expect("権限を読む")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o700);
    }
}
