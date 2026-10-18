//! クライアントが外部のサーバーと読み書きする値（ADR-0005）。
//!
//! サーバー側の [`crate::MappedValue`] は公開する型を Boolean と Double に固定して
//! いるが、クライアントは相手のノードの型をそのまま扱う。型の変換はしない。

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
}
