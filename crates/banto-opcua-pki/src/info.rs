//! async-opcua の X509 から core の [`CertificateInfo`] を作る。

use std::time::{Duration, SystemTime};

use banto_opcua_core::CertificateInfo;
use opcua::crypto::X509;

pub(crate) fn summarize(cert: &X509) -> CertificateInfo {
    CertificateInfo::new(
        thumbprint(cert),
        cert.subject_name(),
        cert.not_before()
            .ok()
            .and_then(|time| system_time(time.timestamp())),
        cert.not_after()
            .ok()
            .and_then(|time| system_time(time.timestamp())),
    )
}

/// 拇印は大文字の 16 進にそろえる（UaExpert などの表示と同じ）。
pub(crate) fn thumbprint(cert: &X509) -> String {
    cert.thumbprint().as_hex_string().to_ascii_uppercase()
}

/// 1970 年より前の時刻は `None`（自己署名の証明書では起こらない）。
fn system_time(unix_seconds: i64) -> Option<SystemTime> {
    let seconds = u64::try_from(unix_seconds).ok()?;
    SystemTime::UNIX_EPOCH.checked_add(Duration::from_secs(seconds))
}

pub(crate) fn invalid_certificate() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        "証明書を DER として読めません",
    )
}
