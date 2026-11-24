//! banto のタグ型から、OPC UA へ公開する DataType と値を決める。
//!
//! 数値のタグは、スケーリングしていなければ元の型（Int16〜UInt64、Float、Double）の
//! まま、スケーリング後の工学値なら Double で公開する。`bit` は Boolean、`string` は
//! String（ADR-0007。2026-10-15 の「数値は Double に固定する」を置き換える）。
//! 値と品質の写像（非有限値・範囲外・型違いは丸めずに値なし・Bad）は
//! [`map_current_value`] と [`map_typed_value`]。

use crate::{Quality, StatusCode, status_code_for_quality};

/// 公開する String の上限（UTF-8 のバイト数）。超えた値は切り詰めずに値なし・Bad にする。
pub const MAX_STRING_BYTES: usize = 4096;

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
    /// タグ型 `i64`。f64 を経由して渡すと 2^53 を超える値は精度を失うので、
    /// 精度が要るなら [`MappedValue::Int64`] で渡す。
    I64,
    /// タグ型 `u64`。`I64` と同じく、精度が要るなら [`MappedValue::UInt64`] で渡す。
    U64,
    /// タグ型 `f32`。
    F32,
    /// タグ型 `f64`。
    F64,
    /// タグ型 `string`。
    String,
}

/// タグの値がスケーリング後の工学値か。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ValueScaling {
    /// 機器から読んだ値のまま。元の型で公開する。
    Raw,
    /// スケーリング後の工学値。小数になり得るので数値は Double で公開する。
    Scaled,
}

/// 公開に使う OPC UA DataType。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DataType {
    /// OPC UA Boolean。`bit` の公開先。
    Boolean,
    /// OPC UA Int16。
    Int16,
    /// OPC UA UInt16。
    UInt16,
    /// OPC UA Int32。
    Int32,
    /// OPC UA UInt32。
    UInt32,
    /// OPC UA Int64。
    Int64,
    /// OPC UA UInt64。
    UInt64,
    /// OPC UA Float。
    Float,
    /// OPC UA Double。スケーリング後の工学値の公開先。
    Double,
    /// OPC UA String。上限は [`MAX_STRING_BYTES`]。
    String,
}

impl std::fmt::Display for DataType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            DataType::Boolean => "Boolean",
            DataType::Int16 => "Int16",
            DataType::UInt16 => "UInt16",
            DataType::Int32 => "Int32",
            DataType::UInt32 => "UInt32",
            DataType::Int64 => "Int64",
            DataType::UInt64 => "UInt64",
            DataType::Float => "Float",
            DataType::Double => "Double",
            DataType::String => "String",
        })
    }
}

/// タグ型とスケーリングの有無から、公開する DataType を決める。
///
/// `Raw` の数値は元の型のまま、`Scaled` の数値は Double。`bit` と `string` は
/// スケーリングに関わらず Boolean と String。
pub const fn publish_data_type(tag_type: TagType, scaling: ValueScaling) -> DataType {
    match (tag_type, scaling) {
        (TagType::Bit, _) => DataType::Boolean,
        (TagType::String, _) => DataType::String,
        (_, ValueScaling::Scaled) => DataType::Double,
        (TagType::I16, ValueScaling::Raw) => DataType::Int16,
        (TagType::U16, ValueScaling::Raw) => DataType::UInt16,
        (TagType::I32, ValueScaling::Raw) => DataType::Int32,
        (TagType::U32, ValueScaling::Raw) => DataType::UInt32,
        (TagType::I64, ValueScaling::Raw) => DataType::Int64,
        (TagType::U64, ValueScaling::Raw) => DataType::UInt64,
        (TagType::F32, ValueScaling::Raw) => DataType::Float,
        (TagType::F64, ValueScaling::Raw) => DataType::Double,
    }
}

/// OPC UA へ渡す、プロトコル実装に依存しない値。型は [`DataType`] と 1 対 1。
#[derive(Debug, Clone, PartialEq)]
pub enum MappedValue {
    /// Boolean。
    Boolean(bool),
    /// Int16。
    Int16(i16),
    /// UInt16。
    UInt16(u16),
    /// Int32。
    Int32(i32),
    /// UInt32。
    UInt32(u32),
    /// Int64。
    Int64(i64),
    /// UInt64。
    UInt64(u64),
    /// Float。写像を通った値は有限値に限る。
    Float(f32),
    /// Double。写像を通った値は有限値に限る。
    Double(f64),
    /// String。写像を通った値は [`MAX_STRING_BYTES`] 以下に限る。
    String(String),
}

impl MappedValue {
    /// 値の型。
    pub const fn data_type(&self) -> DataType {
        match self {
            Self::Boolean(_) => DataType::Boolean,
            Self::Int16(_) => DataType::Int16,
            Self::UInt16(_) => DataType::UInt16,
            Self::Int32(_) => DataType::Int32,
            Self::UInt32(_) => DataType::UInt32,
            Self::Int64(_) => DataType::Int64,
            Self::UInt64(_) => DataType::UInt64,
            Self::Float(_) => DataType::Float,
            Self::Double(_) => DataType::Double,
            Self::String(_) => DataType::String,
        }
    }

    /// ハブの f64 の値を、公開する型の値へ変える。表せなければ `None`。
    ///
    /// 丸めない。Boolean は 0 / 1（`-0.0` は false）、整数の型はその型の範囲の
    /// 整数、Float は f32 で正確に表せる有限値（f32 から広げた値。f32 にして f64 へ
    /// 戻すと元の値に戻るもの）、Double は有限値だけを受ける。f64 の 0.1 や、f32 では
    /// 0 に落ちる 1e-50 は Float にしない。String は
    /// 数値から作らないので常に `None`。
    pub fn from_number(data_type: DataType, number: f64) -> Option<Self> {
        if !number.is_finite() {
            return None;
        }
        let integral = number.fract() == 0.0;
        // 2^63 と 2^64 は f64 で正確に表せる。上限は含まない。
        const I64_END: f64 = 9_223_372_036_854_775_808.0;
        const U64_END: f64 = 18_446_744_073_709_551_616.0;
        Some(match data_type {
            DataType::Boolean if number == 0.0 => Self::Boolean(false),
            DataType::Boolean if number == 1.0 => Self::Boolean(true),
            DataType::Int16
                if integral && (f64::from(i16::MIN)..=f64::from(i16::MAX)).contains(&number) =>
            {
                Self::Int16(number as i16)
            }
            DataType::UInt16 if integral && (0.0..=f64::from(u16::MAX)).contains(&number) => {
                Self::UInt16(number as u16)
            }
            DataType::Int32
                if integral && (f64::from(i32::MIN)..=f64::from(i32::MAX)).contains(&number) =>
            {
                Self::Int32(number as i32)
            }
            DataType::UInt32 if integral && (0.0..=f64::from(u32::MAX)).contains(&number) => {
                Self::UInt32(number as u32)
            }
            DataType::Int64 if integral && (-I64_END..I64_END).contains(&number) => {
                Self::Int64(number as i64)
            }
            DataType::UInt64 if integral && (0.0..U64_END).contains(&number) => {
                Self::UInt64(number as u64)
            }
            // 範囲外は inf、表せない値は近い値か 0 に丸まるので、往復で一致するものだけ。
            DataType::Float if f64::from(number as f32) == number => Self::Float(number as f32),
            DataType::Double => Self::Double(number),
            _ => return None,
        })
    }

    /// `data_type` のノードへそのまま公開できる値か。型が一致し、Float / Double は
    /// 有限値、String は [`MAX_STRING_BYTES`] 以下であること。
    pub fn is_publishable_as(&self, data_type: DataType) -> bool {
        self.data_type() == data_type
            && match self {
                Self::Float(number) => number.is_finite(),
                Self::Double(number) => number.is_finite(),
                Self::String(text) => text.len() <= MAX_STRING_BYTES,
                _ => true,
            }
    }
}

/// 値だけを書く（Rust の各型の `Display` と同じ。String は引用符を付けない）。
impl std::fmt::Display for MappedValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Boolean(value) => value.fmt(f),
            Self::Int16(value) => value.fmt(f),
            Self::UInt16(value) => value.fmt(f),
            Self::Int32(value) => value.fmt(f),
            Self::UInt32(value) => value.fmt(f),
            Self::Int64(value) => value.fmt(f),
            Self::UInt64(value) => value.fmt(f),
            Self::Float(value) => value.fmt(f),
            Self::Double(value) => value.fmt(f),
            Self::String(value) => value.fmt(f),
        }
    }
}

/// 現在値と品質を写した結果。サーバー側で DataValue に変換する。
/// #8 の時刻などを後から追加できるよう、外部クレートでの構造体リテラルは認めない。
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct MappedReading {
    /// 無効な値、欠損値、Bad 品質では `None`。
    pub value: Option<MappedValue>,
    /// 値と整合する OPC UA StatusCode。
    pub status: StatusCode,
}

/// ハブの `Option<f64>` と品質を、公開先の型に従って写す。
///
/// f64 から型の値への変換は [`MappedValue::from_number`]（丸めない）。変換できない
/// 値は値なし・汎用 Bad にする。String のタグには [`map_typed_value`] を使う。
/// 品質の扱いは [`map_typed_value`] と同じ。
pub fn map_current_value(
    data_type: DataType,
    value: Option<f64>,
    quality: Quality,
) -> MappedReading {
    let converted = value.map(|number| MappedValue::from_number(data_type, number));
    match converted {
        Some(None) => MappedReading {
            value: None,
            status: StatusCode::BAD,
        },
        Some(converted) => map_typed_value(data_type, converted, quality),
        None => map_typed_value(data_type, None, quality),
    }
}

/// 型の付いた値と品質を、公開先の型に従って写す。
///
/// 品質が Bad、または値がない場合は値なし・Bad。型が `data_type` と違う値、
/// 非有限の Float / Double、[`MAX_STRING_BYTES`] を超える String も丸めず・
/// 切り詰めずに値なし・Bad にする。有効な値の Stale は最後の値を保持する。
/// Stale でも値が無効なら値なし・Bad を優先する。
pub fn map_typed_value(
    data_type: DataType,
    value: Option<MappedValue>,
    quality: Quality,
) -> MappedReading {
    match value {
        Some(value) if quality != Quality::Bad && value.is_publishable_as(data_type) => {
            MappedReading {
                value: Some(value),
                status: status_code_for_quality(quality),
            }
        }
        _ => MappedReading {
            value: None,
            status: StatusCode::BAD,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NUMERIC: [(TagType, DataType); 8] = [
        (TagType::I16, DataType::Int16),
        (TagType::U16, DataType::UInt16),
        (TagType::I32, DataType::Int32),
        (TagType::U32, DataType::UInt32),
        (TagType::I64, DataType::Int64),
        (TagType::U64, DataType::UInt64),
        (TagType::F32, DataType::Float),
        (TagType::F64, DataType::Double),
    ];

    fn bad() -> MappedReading {
        MappedReading {
            value: None,
            status: StatusCode::BAD,
        }
    }

    #[test]
    fn raw_numbers_publish_as_their_own_type() {
        for (tag_type, data_type) in NUMERIC {
            assert_eq!(
                publish_data_type(tag_type, ValueScaling::Raw),
                data_type,
                "{tag_type:?}"
            );
        }
    }

    #[test]
    fn scaled_numbers_publish_as_double() {
        for (tag_type, _) in NUMERIC {
            assert_eq!(
                publish_data_type(tag_type, ValueScaling::Scaled),
                DataType::Double,
                "{tag_type:?}"
            );
        }
    }

    #[test]
    fn bit_and_string_ignore_scaling() {
        for scaling in [ValueScaling::Raw, ValueScaling::Scaled] {
            assert_eq!(publish_data_type(TagType::Bit, scaling), DataType::Boolean);
            assert_eq!(
                publish_data_type(TagType::String, scaling),
                DataType::String
            );
        }
    }

    #[test]
    fn data_type_names_match_the_opc_ua_built_in_types() {
        for data_type in [
            DataType::Boolean,
            DataType::Int16,
            DataType::UInt16,
            DataType::Int32,
            DataType::UInt32,
            DataType::Int64,
            DataType::UInt64,
            DataType::Float,
            DataType::Double,
            DataType::String,
        ] {
            assert_eq!(data_type.to_string(), format!("{data_type:?}"));
        }
    }

    #[test]
    fn numbers_convert_without_rounding_within_the_type_range() {
        let cases = [
            (
                DataType::Int16,
                -32768.0,
                Some(MappedValue::Int16(i16::MIN)),
            ),
            (DataType::Int16, 32767.0, Some(MappedValue::Int16(i16::MAX))),
            (DataType::Int16, 32768.0, None),
            (DataType::Int16, 1.5, None),
            (
                DataType::UInt16,
                65535.0,
                Some(MappedValue::UInt16(u16::MAX)),
            ),
            (DataType::UInt16, -1.0, None),
            (DataType::UInt16, -0.0, Some(MappedValue::UInt16(0))),
            (
                DataType::Int32,
                -2_147_483_648.0,
                Some(MappedValue::Int32(i32::MIN)),
            ),
            (DataType::Int32, 2_147_483_648.0, None),
            (
                DataType::UInt32,
                4_294_967_295.0,
                Some(MappedValue::UInt32(u32::MAX)),
            ),
            (DataType::UInt32, 4_294_967_296.0, None),
            (
                DataType::Int64,
                -9_223_372_036_854_775_808.0,
                Some(MappedValue::Int64(i64::MIN)),
            ),
            (DataType::Int64, 9_223_372_036_854_775_808.0, None),
            (
                DataType::UInt64,
                9_007_199_254_740_993.0,
                Some(MappedValue::UInt64(9_007_199_254_740_992)),
            ),
            (DataType::UInt64, 18_446_744_073_709_551_616.0, None),
            (DataType::Float, 1.5, Some(MappedValue::Float(1.5))),
            (DataType::Float, -0.0, Some(MappedValue::Float(-0.0))),
            (
                DataType::Float,
                f64::from(0.1_f32),
                Some(MappedValue::Float(0.1)),
            ),
            (
                DataType::Float,
                f64::from(f32::MAX),
                Some(MappedValue::Float(f32::MAX)),
            ),
            (
                DataType::Float,
                f64::from(f32::from_bits(1)),
                Some(MappedValue::Float(f32::from_bits(1))),
            ),
            (DataType::Float, 0.1, None),
            (DataType::Float, 1e-50, None),
            (DataType::Float, f64::from(f32::MAX) * 2.0, None),
            (DataType::Double, 12.5, Some(MappedValue::Double(12.5))),
            (DataType::Boolean, -0.0, Some(MappedValue::Boolean(false))),
            (DataType::Boolean, 1.0, Some(MappedValue::Boolean(true))),
            (DataType::Boolean, 0.5, None),
            (DataType::String, 1.0, None),
        ];
        for (data_type, number, expected) in cases {
            assert_eq!(
                MappedValue::from_number(data_type, number),
                expected,
                "{data_type} {number}"
            );
        }
        for data_type in [
            DataType::Int16,
            DataType::UInt64,
            DataType::Float,
            DataType::Double,
        ] {
            for number in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
                assert_eq!(MappedValue::from_number(data_type, number), None);
            }
        }
    }

    #[test]
    fn nonfinite_and_out_of_range_numbers_have_no_value_and_bad_status() {
        for (data_type, value) in [
            (DataType::Double, f64::NAN),
            (DataType::Double, f64::INFINITY),
            (DataType::Float, f64::NEG_INFINITY),
            (DataType::Int16, 1.5),
            (DataType::UInt32, -1.0),
            (DataType::Boolean, 2.0),
            (DataType::String, 0.0),
        ] {
            for quality in [Quality::Good, Quality::Stale] {
                assert_eq!(
                    map_current_value(data_type, Some(value), quality),
                    bad(),
                    "{data_type} {value} {quality:?}"
                );
            }
        }
        assert_eq!(
            map_current_value(DataType::Double, Some(12.5), Quality::Good),
            MappedReading {
                value: Some(MappedValue::Double(12.5)),
                status: StatusCode::GOOD,
            }
        );
        assert_eq!(
            map_current_value(DataType::Int32, Some(-7.0), Quality::Good),
            MappedReading {
                value: Some(MappedValue::Int32(-7)),
                status: StatusCode::GOOD,
            }
        );
    }

    #[test]
    fn missing_or_bad_quality_has_no_value_and_stale_keeps_last_usable_value() {
        for quality in [Quality::Good, Quality::Stale, Quality::Bad] {
            assert_eq!(map_current_value(DataType::Double, None, quality), bad());
            assert_eq!(map_typed_value(DataType::String, None, quality), bad());
        }
        assert_eq!(
            map_current_value(DataType::Double, Some(42.0), Quality::Bad),
            bad()
        );
        assert_eq!(
            map_current_value(DataType::Boolean, Some(1.0), Quality::Bad),
            bad()
        );
        assert_eq!(
            map_current_value(DataType::Double, Some(42.0), Quality::Stale),
            MappedReading {
                value: Some(MappedValue::Double(42.0)),
                status: StatusCode::UNCERTAIN_LAST_USABLE_VALUE,
            }
        );
        assert_eq!(
            map_current_value(DataType::Boolean, Some(1.0), Quality::Stale),
            MappedReading {
                value: Some(MappedValue::Boolean(true)),
                status: StatusCode::UNCERTAIN_LAST_USABLE_VALUE,
            }
        );
    }

    #[test]
    fn typed_values_keep_64_bit_precision() {
        let counter = (1_u64 << 53) + 1;
        assert_eq!(
            map_typed_value(
                DataType::UInt64,
                Some(MappedValue::UInt64(counter)),
                Quality::Good
            ),
            MappedReading {
                value: Some(MappedValue::UInt64(counter)),
                status: StatusCode::GOOD,
            }
        );
        assert_eq!(
            map_typed_value(
                DataType::Int64,
                Some(MappedValue::Int64(i64::MIN)),
                Quality::Stale
            ),
            MappedReading {
                value: Some(MappedValue::Int64(i64::MIN)),
                status: StatusCode::UNCERTAIN_LAST_USABLE_VALUE,
            }
        );
    }

    #[test]
    fn typed_values_of_another_type_or_invalid_have_no_value_and_bad_status() {
        for (data_type, value) in [
            (DataType::Double, MappedValue::Int32(1)),
            (DataType::Int64, MappedValue::UInt64(1)),
            (DataType::Float, MappedValue::Float(f32::NAN)),
            (DataType::Double, MappedValue::Double(f64::INFINITY)),
            (
                DataType::String,
                MappedValue::String("a".repeat(MAX_STRING_BYTES + 1)),
            ),
        ] {
            for quality in [Quality::Good, Quality::Stale] {
                assert_eq!(
                    map_typed_value(data_type, Some(value.clone()), quality),
                    bad(),
                    "{data_type} {value:?} {quality:?}"
                );
            }
        }
    }

    #[test]
    fn strings_up_to_the_limit_are_published_as_is() {
        // 上限は文字数ではなくバイト数。3 バイトの文字と 1 バイトの文字で上限ちょうどにする。
        let text = "あ".repeat(MAX_STRING_BYTES / 3) + "a";
        assert_eq!(text.len(), MAX_STRING_BYTES);
        for (quality, status) in [
            (Quality::Good, StatusCode::GOOD),
            (Quality::Stale, StatusCode::UNCERTAIN_LAST_USABLE_VALUE),
        ] {
            assert_eq!(
                map_typed_value(
                    DataType::String,
                    Some(MappedValue::String(text.clone())),
                    quality
                ),
                MappedReading {
                    value: Some(MappedValue::String(text.clone())),
                    status,
                }
            );
        }
        assert_eq!(
            map_typed_value(
                DataType::String,
                Some(MappedValue::String(String::new())),
                Quality::Good
            )
            .value,
            Some(MappedValue::String(String::new()))
        );
    }
}
