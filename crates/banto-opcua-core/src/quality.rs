//! banto-hub の品質（Good / Bad / Stale）を OPC UA StatusCode へ写す。
//!
//! 数値は OPC 10000-6 の StatusCode 表、async-opcua 0.19 の定数と一致する
//! （サーバークレートの単体テストが突き合わせる）。Bad の細分化はしない
//! （2026-10-15 オーナー決定）。Good / Bad / Stale の 3 値を維持する。

/// 収集側の品質。banto-hub の現在値は Good / Bad / Stale の 3 値。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Quality {
    /// 直近の取得が成功している。
    Good,
    /// 取得に失敗している、または値が無い。原因（通信断かタグ単位か）は分けない。
    Bad,
    /// 以前の値は残っているが、更新が止まっている。
    Stale,
}

/// OPC UA StatusCode の、このライブラリが名前を持つ部分集合。
///
/// 未知のコードも数値のまま保持できる。深刻度ビット（上位 2 ビット）は
/// OPC UA の規定どおり Good = 00、Uncertain = 01、Bad = 10。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StatusCode(u32);

impl StatusCode {
    /// `Good`（0x00000000）。
    pub const GOOD: Self = Self(0x0000_0000);
    /// `Bad`（0x80000000）。原因を特定しない失敗。
    pub const BAD: Self = Self(0x8000_0000);
    /// `UncertainLastUsableValue`（0x40900000）。更新が止まった最後の値。
    pub const UNCERTAIN_LAST_USABLE_VALUE: Self = Self(0x4090_0000);
    /// `BadNodeIdUnknown`（0x80340000）。削除されたノードの購読へ返す候補。
    pub const BAD_NODE_ID_UNKNOWN: Self = Self(0x8034_0000);

    /// 32 ビットの生値を返す。
    pub const fn code(self) -> u32 {
        self.0
    }

    /// 生値から作る。既知の定数以外も受け取れる。
    pub const fn from_code(code: u32) -> Self {
        Self(code)
    }

    /// 深刻度が Good か。
    pub const fn is_good(self) -> bool {
        self.0 & 0xC000_0000 == 0
    }

    /// 深刻度が Uncertain か。
    pub const fn is_uncertain(self) -> bool {
        self.0 & 0xC000_0000 == 0x4000_0000
    }

    /// 深刻度が Bad か。
    pub const fn is_bad(self) -> bool {
        self.0 & 0xC000_0000 == 0x8000_0000
    }
}

impl std::fmt::Display for StatusCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self.0 {
            0x0000_0000 => "Good",
            0x8000_0000 => "Bad",
            0x4090_0000 => "UncertainLastUsableValue",
            0x8034_0000 => "BadNodeIdUnknown",
            _ => return write!(f, "StatusCode(0x{:08X})", self.0),
        };
        f.write_str(name)
    }
}

/// 品質を StatusCode へ写す。Bad の細分化はしない（2026-10-15 オーナー決定）。
pub const fn status_code_for_quality(quality: Quality) -> StatusCode {
    match quality {
        Quality::Good => StatusCode::GOOD,
        Quality::Bad => StatusCode::BAD,
        Quality::Stale => StatusCode::UNCERTAIN_LAST_USABLE_VALUE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn good_bad_and_stale_map_to_status_codes() {
        assert_eq!(status_code_for_quality(Quality::Good), StatusCode::GOOD);
        assert_eq!(status_code_for_quality(Quality::Bad), StatusCode::BAD);
        assert_eq!(
            status_code_for_quality(Quality::Stale),
            StatusCode::UNCERTAIN_LAST_USABLE_VALUE
        );
    }

    #[test]
    fn severity_bits_match_the_opc_ua_classes() {
        assert!(StatusCode::GOOD.is_good());
        assert!(StatusCode::UNCERTAIN_LAST_USABLE_VALUE.is_uncertain());
        assert!(StatusCode::BAD.is_bad());
        assert!(StatusCode::BAD_NODE_ID_UNKNOWN.is_bad());
        assert!(!StatusCode::BAD.is_uncertain());
    }

    #[test]
    fn display_uses_the_symbolic_name_for_known_codes() {
        assert_eq!(StatusCode::GOOD.to_string(), "Good");
        assert_eq!(
            StatusCode::UNCERTAIN_LAST_USABLE_VALUE.to_string(),
            "UncertainLastUsableValue"
        );
        assert_eq!(
            StatusCode::from_code(0x1).to_string(),
            "StatusCode(0x00000001)"
        );
    }
}
