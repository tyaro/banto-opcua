//! クライアントが外部のサーバーと読み書きする値（ADR-0005）。
//!
//! クライアントは相手のノードの型をそのまま扱う。型の変換はしない。サーバー側の
//! [`crate::MappedValue`] とは SByte と Byte の有無だけが違い、互いの変換は
//! `From` / `TryFrom` で行う（ADR-0007）。

use crate::MappedValue;

/// OPC UA のスカラーの組み込み型のうち、クライアントが扱う値。
///
/// 配列とここに無い型は「対応しない値」として、クライアントがノード単位の Bad にする。
#[derive(Debug, Clone, PartialEq)]
pub enum ScalarValue {
    /// Boolean。
    Boolean(bool),
    /// SByte。
    SByte(i8),
    /// Byte。
    Byte(u8),
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
    /// Float。非有限値もそのまま持つ。
    Float(f32),
    /// Double。非有限値もそのまま持つ。
    Double(f64),
    /// String。
    String(String),
}

impl ScalarValue {
    /// 値の型。
    pub const fn scalar_type(&self) -> ScalarType {
        match self {
            Self::Boolean(_) => ScalarType::Boolean,
            Self::SByte(_) => ScalarType::SByte,
            Self::Byte(_) => ScalarType::Byte,
            Self::Int16(_) => ScalarType::Int16,
            Self::UInt16(_) => ScalarType::UInt16,
            Self::Int32(_) => ScalarType::Int32,
            Self::UInt32(_) => ScalarType::UInt32,
            Self::Int64(_) => ScalarType::Int64,
            Self::UInt64(_) => ScalarType::UInt64,
            Self::Float(_) => ScalarType::Float,
            Self::Double(_) => ScalarType::Double,
            Self::String(_) => ScalarType::String,
        }
    }
}

/// [`ScalarValue`] の型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ScalarType {
    /// Boolean。
    Boolean,
    /// SByte。
    SByte,
    /// Byte。
    Byte,
    /// Int16。
    Int16,
    /// UInt16。
    UInt16,
    /// Int32。
    Int32,
    /// UInt32。
    UInt32,
    /// Int64。
    Int64,
    /// UInt64。
    UInt64,
    /// Float。
    Float,
    /// Double。
    Double,
    /// String。
    String,
}

impl std::fmt::Display for ScalarType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Boolean => "Boolean",
            Self::SByte => "SByte",
            Self::Byte => "Byte",
            Self::Int16 => "Int16",
            Self::UInt16 => "UInt16",
            Self::Int32 => "Int32",
            Self::UInt32 => "UInt32",
            Self::Int64 => "Int64",
            Self::UInt64 => "UInt64",
            Self::Float => "Float",
            Self::Double => "Double",
            Self::String => "String",
        })
    }
}

impl From<MappedValue> for ScalarValue {
    fn from(value: MappedValue) -> Self {
        match value {
            MappedValue::Boolean(value) => Self::Boolean(value),
            MappedValue::Int16(value) => Self::Int16(value),
            MappedValue::UInt16(value) => Self::UInt16(value),
            MappedValue::Int32(value) => Self::Int32(value),
            MappedValue::UInt32(value) => Self::UInt32(value),
            MappedValue::Int64(value) => Self::Int64(value),
            MappedValue::UInt64(value) => Self::UInt64(value),
            MappedValue::Float(value) => Self::Float(value),
            MappedValue::Double(value) => Self::Double(value),
            MappedValue::String(value) => Self::String(value),
        }
    }
}

/// サーバーが公開しない型（SByte と Byte）の値。持っているのはその型。
///
/// 型を黙って広げない。広げるなら呼び出し側が型を決めて変換する。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NotPublishable(pub ScalarType);

impl std::fmt::Display for NotPublishable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} はサーバーが公開しない型です", self.0)
    }
}

impl std::error::Error for NotPublishable {}

impl TryFrom<ScalarValue> for MappedValue {
    type Error = NotPublishable;

    fn try_from(value: ScalarValue) -> Result<Self, Self::Error> {
        Ok(match value {
            ScalarValue::Boolean(value) => Self::Boolean(value),
            ScalarValue::Int16(value) => Self::Int16(value),
            ScalarValue::UInt16(value) => Self::UInt16(value),
            ScalarValue::Int32(value) => Self::Int32(value),
            ScalarValue::UInt32(value) => Self::UInt32(value),
            ScalarValue::Int64(value) => Self::Int64(value),
            ScalarValue::UInt64(value) => Self::UInt64(value),
            ScalarValue::Float(value) => Self::Float(value),
            ScalarValue::Double(value) => Self::Double(value),
            ScalarValue::String(value) => Self::String(value),
            ScalarValue::SByte(_) | ScalarValue::Byte(_) => {
                return Err(NotPublishable(value.scalar_type()));
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalar_type_names_match_the_opc_ua_built_in_types() {
        let cases = [
            (ScalarValue::Boolean(true), "Boolean"),
            (ScalarValue::SByte(-1), "SByte"),
            (ScalarValue::Byte(1), "Byte"),
            (ScalarValue::Int16(-1), "Int16"),
            (ScalarValue::UInt16(1), "UInt16"),
            (ScalarValue::Int32(-1), "Int32"),
            (ScalarValue::UInt32(1), "UInt32"),
            (ScalarValue::Int64(-1), "Int64"),
            (ScalarValue::UInt64(1), "UInt64"),
            (ScalarValue::Float(1.5), "Float"),
            (ScalarValue::Double(1.5), "Double"),
            (ScalarValue::String("a".into()), "String"),
        ];
        let names: std::collections::HashSet<_> = cases
            .iter()
            .map(|(value, name)| {
                assert_eq!(value.scalar_type().to_string(), *name);
                value.scalar_type()
            })
            .collect();
        assert_eq!(names.len(), cases.len());
    }

    #[test]
    fn mapped_values_round_trip_through_scalar_values() {
        for value in [
            MappedValue::Boolean(true),
            MappedValue::Int16(-1),
            MappedValue::UInt16(1),
            MappedValue::Int32(-1),
            MappedValue::UInt32(1),
            MappedValue::Int64(i64::MIN),
            MappedValue::UInt64(u64::MAX),
            MappedValue::Float(1.5),
            MappedValue::Double(1.5),
            MappedValue::String("a".into()),
        ] {
            let scalar = ScalarValue::from(value.clone());
            assert_eq!(
                scalar.scalar_type().to_string(),
                value.data_type().to_string()
            );
            assert_eq!(MappedValue::try_from(scalar), Ok(value));
        }
    }

    #[test]
    fn sbyte_and_byte_are_not_widened() {
        assert_eq!(
            MappedValue::try_from(ScalarValue::SByte(-1)),
            Err(NotPublishable(ScalarType::SByte))
        );
        assert_eq!(
            MappedValue::try_from(ScalarValue::Byte(1)),
            Err(NotPublishable(ScalarType::Byte))
        );
    }
}
