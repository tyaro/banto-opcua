//! banto のタグ型から、OPC UA へ公開する DataType を決める。
//!
//! 2026-10-15 のオーナー決定。現在値はスケーリング後の工学値なので、宣言型の
//! 整数へキャストしない。収集済みの数値は Double に固定する。`bit` は
//! Boolean、`string` は周期収集の対象外なので値を公開しない。

use crate::{Quality, StatusCode, status_code_for_quality};

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

/// 公開に使う OPC UA DataType。収集済みの数値は Double に固定する。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DataType {
    /// OPC UA Boolean。`bit` の公開先。
    Boolean,
    /// OPC UA Double。収集済み数値の公開先。
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
    /// 値を公開しない。`string` は周期収集の対象外。
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

/// OPC UA へ渡す、プロトコル実装に依存しない現在値。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MappedValue {
    /// `bit` の 0 / 1 から作る Boolean。
    Boolean(bool),
    /// 収集済み数値の Double。非有限値は含まない。
    Double(f64),
}

/// 現在値と品質を写した結果。サーバー側で DataValue に変換する。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MappedReading {
    /// 無効な値、欠損値、Bad 品質では `None`。
    pub value: Option<MappedValue>,
    /// 値と整合する OPC UA StatusCode。
    pub status: StatusCode,
}

/// ハブの `Option<f64>` と品質を、公開先の型に従って写す。
///
/// 呼び出し側は先に [`publish_decision`] で公開対象を選ぶ。非有限値と 0 / 1
/// 以外の bit は丸めず、値なし・汎用 Bad にする。品質が Bad、または値がない
/// 場合も値なし・Bad にする。有効な値の Stale は最後の値を保持する。
pub fn map_current_value(
    data_type: DataType,
    value: Option<f64>,
    quality: Quality,
) -> MappedReading {
    if quality == Quality::Bad {
        return MappedReading {
            value: None,
            status: StatusCode::BAD,
        };
    }

    let mapped = match (data_type, value) {
        (DataType::Boolean, Some(0.0)) => Some(MappedValue::Boolean(false)),
        (DataType::Boolean, Some(1.0)) => Some(MappedValue::Boolean(true)),
        (DataType::Double, Some(number)) if number.is_finite() => Some(MappedValue::Double(number)),
        _ => None,
    };

    MappedReading {
        value: mapped,
        status: if mapped.is_some() {
            status_code_for_quality(quality)
        } else {
            StatusCode::BAD
        },
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

    #[test]
    fn double_rejects_nonfinite_values_without_good_status() {
        for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(
                map_current_value(DataType::Double, Some(value), Quality::Good),
                MappedReading {
                    value: None,
                    status: StatusCode::BAD,
                }
            );
        }
        assert_eq!(
            map_current_value(DataType::Double, Some(12.5), Quality::Good),
            MappedReading {
                value: Some(MappedValue::Double(12.5)),
                status: StatusCode::GOOD,
            }
        );
    }

    #[test]
    fn bit_accepts_only_zero_and_one() {
        for (input, expected) in [
            (0.0, Some(false)),
            (1.0, Some(true)),
            (0.5, None),
            (2.0, None),
        ] {
            let reading = map_current_value(DataType::Boolean, Some(input), Quality::Good);
            assert_eq!(reading.value, expected.map(MappedValue::Boolean), "{input}");
            assert_eq!(
                reading.status,
                if expected.is_some() {
                    StatusCode::GOOD
                } else {
                    StatusCode::BAD
                },
                "{input}"
            );
        }
    }

    #[test]
    fn missing_or_bad_quality_has_no_value_and_stale_keeps_last_usable_value() {
        for quality in [Quality::Good, Quality::Stale, Quality::Bad] {
            assert_eq!(
                map_current_value(DataType::Double, None, quality),
                MappedReading {
                    value: None,
                    status: StatusCode::BAD,
                }
            );
        }
        assert_eq!(
            map_current_value(DataType::Double, Some(42.0), Quality::Bad),
            MappedReading {
                value: None,
                status: StatusCode::BAD,
            }
        );
        assert_eq!(
            map_current_value(DataType::Double, Some(42.0), Quality::Stale),
            MappedReading {
                value: Some(MappedValue::Double(42.0)),
                status: StatusCode::UNCERTAIN_LAST_USABLE_VALUE,
            }
        );
    }
}
