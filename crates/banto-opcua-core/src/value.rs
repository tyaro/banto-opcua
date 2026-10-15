//! banto のタグ型から、OPC UA へ公開する DataType を決める。
//!
//! 2026-10-11 のレビュー（tyaro/banto-industrial#201）の最小案:
//! 現在値はスケーリング後の工学値なので、宣言型の整数へキャストしない。
//! `bit` は Boolean、収集済みの数値は Double、`string` は周期収集の対象外
//! なので値公開しない。これは足場であり、オーナー決定ではない。

/// banto-hub のタグ型名に対応する分類。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TagType {
    /// タグ型 `bit`。
    Bit,
    /// タグ型 `i16`。
    I16,
    /// タグ型 `u16`。
    U16,
    /// タグ型 `i32`。
    I32,
    /// タグ型 `u32`。
    U32,
    /// タグ型 `i64`。内部では f64 を経由し、2^53 を超えると精度を失う（既知の制限）。
    I64,
    /// タグ型 `u64`。`I64` と同じ精度の制限がある。
    U64,
    /// タグ型 `f32`。
    F32,
    /// タグ型 `f64`。
    F64,
    /// タグ型 `string`。周期収集・現在値キャッシュの対象外。
    String,
}

/// 公開に使う OPC UA DataType の足場。数値の細分化はまだしない。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DataType {
    /// OPC UA Boolean。`bit` の公開先。
    Boolean,
    /// OPC UA Double。収集済み数値の公開先候補。
    Double,
}

impl std::fmt::Display for DataType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            DataType::Boolean => "Boolean",
            DataType::Double => "Double",
        })
    }
}

/// そのタグ型を値として公開するか。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PublishDecision {
    /// この DataType で公開する。
    Publish(DataType),
    /// 値公開の対象外。ノードを作るかどうかは呼び出し側の未決事項。
    NotCollected,
}

/// タグ型を公開判定へ写す。
pub const fn publish_decision(tag_type: TagType) -> PublishDecision {
    match tag_type {
        TagType::Bit => PublishDecision::Publish(DataType::Boolean),
        TagType::String => PublishDecision::NotCollected,
        TagType::I16
        | TagType::U16
        | TagType::I32
        | TagType::U32
        | TagType::I64
        | TagType::U64
        | TagType::F32
        | TagType::F64 => PublishDecision::Publish(DataType::Double),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bit_publishes_as_boolean() {
        assert_eq!(
            publish_decision(TagType::Bit),
            PublishDecision::Publish(DataType::Boolean)
        );
    }

    #[test]
    fn collected_numbers_publish_as_double() {
        for tag_type in [
            TagType::I16,
            TagType::U16,
            TagType::I32,
            TagType::U32,
            TagType::I64,
            TagType::U64,
            TagType::F32,
            TagType::F64,
        ] {
            assert_eq!(
                publish_decision(tag_type),
                PublishDecision::Publish(DataType::Double),
                "{tag_type:?}"
            );
        }
    }

    #[test]
    fn string_is_not_collected() {
        assert_eq!(
            publish_decision(TagType::String),
            PublishDecision::NotCollected
        );
    }
}
