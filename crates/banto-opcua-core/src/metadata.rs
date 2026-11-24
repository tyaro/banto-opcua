//! タグの変数ノードに付ける説明・単位・レンジ・二値の文言（ADR-0008）。
//!
//! 値とは別に、記録やトレンドの軸・帳票の見出しに使う属性。サーバーは
//! Description 属性と、EngineeringUnits・EURange・TrueState・FalseState の
//! プロパティに写す。どのプロパティを持つか（ノードの形）は [`MetadataShape`]。

use crate::{DataType, MAX_STRING_BYTES};

/// UNECE の単位のコードの名前空間（OPC 10000-8 の EUInformation）。
pub const UNECE_UNITS_NAMESPACE_URI: &str = "http://www.opcfoundation.org/UA/units/un/cefact";

/// 単位。EUInformation に写す。
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct EngineeringUnits {
    /// 表示の文字列（`℃`、`kPa` など）。
    pub display_name: String,
    /// 単位の説明。無ければ空の LocalizedText。
    pub description: Option<String>,
    /// UNECE Recommendation 20 の Common Code（`CEL`、`KPA` など）。無ければ UnitId は -1。
    pub unece_code: Option<String>,
}

impl EngineeringUnits {
    /// 表示の文字列だけの単位。UNECE のコードは持たない（UnitId は -1）。
    pub fn display(display_name: impl Into<String>) -> Self {
        Self {
            display_name: display_name.into(),
            description: None,
            unece_code: None,
        }
    }

    /// UNECE の Common Code を付ける。
    pub fn with_unece_code(mut self, code: impl Into<String>) -> Self {
        self.unece_code = Some(code.into());
        self
    }

    /// 単位の説明を付ける。
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// EUInformation の UnitId。UNECE のコードが無ければ -1。
    ///
    /// OPC 10000-8 の算法どおり、コードの各文字（ASCII）を上位から 8 ビットずつ詰める。
    /// コードが不正なら `None`（[`TagMetadata::validate`] が先に断る）。
    pub fn unit_id(&self) -> Option<i32> {
        let Some(code) = &self.unece_code else {
            return Some(-1);
        };
        if code.is_empty()
            || code.len() > 3
            || !code
                .bytes()
                .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit())
        {
            return None;
        }
        Some(
            code.bytes()
                .fold(0_i32, |id, byte| (id << 8) | i32::from(byte)),
        )
    }
}

/// 計器のレンジ。EURange（Range）に写す。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EuRange {
    /// 下限。
    pub low: f64,
    /// 上限。
    pub high: f64,
}

impl EuRange {
    /// レンジを作る。検査は [`TagMetadata::validate`]。
    pub const fn new(low: f64, high: f64) -> Self {
        Self { low, high }
    }
}

/// bit の 1 / 0 の文言。TrueState・FalseState に写す。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TwoStateLabels {
    /// true（1）の文言（`運転` など）。
    pub true_state: String,
    /// false（0）の文言（`停止` など）。
    pub false_state: String,
}

impl TwoStateLabels {
    /// 文言の組を作る。
    pub fn new(true_state: impl Into<String>, false_state: impl Into<String>) -> Self {
        Self {
            true_state: true_state.into(),
            false_state: false_state.into(),
        }
    }
}

/// タグの変数ノードに付ける属性。どれも任意。
#[derive(Debug, Clone, Default, PartialEq)]
#[non_exhaustive]
pub struct TagMetadata {
    /// Description 属性。
    pub description: Option<String>,
    /// EngineeringUnits プロパティ。数値のタグだけ。
    pub engineering_units: Option<EngineeringUnits>,
    /// EURange プロパティ。数値のタグだけ。
    pub eu_range: Option<EuRange>,
    /// TrueState・FalseState プロパティ。Boolean のタグだけ。
    pub two_state: Option<TwoStateLabels>,
}

impl TagMetadata {
    /// 属性の無いもの。
    pub fn new() -> Self {
        Self::default()
    }

    /// Description を付ける。
    pub fn with_description(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// 単位を付ける。
    pub fn with_engineering_units(mut self, units: EngineeringUnits) -> Self {
        self.engineering_units = Some(units);
        self
    }

    /// レンジを付ける。
    pub fn with_eu_range(mut self, range: EuRange) -> Self {
        self.eu_range = Some(range);
        self
    }

    /// 二値の文言を付ける。
    pub fn with_two_state(mut self, labels: TwoStateLabels) -> Self {
        self.two_state = Some(labels);
        self
    }

    /// `data_type` のタグに付けられるかを確かめる。
    pub fn validate(&self, data_type: DataType) -> Result<(), MetadataError> {
        let numeric = !matches!(data_type, DataType::Boolean | DataType::String);
        let texts = [
            self.description.as_deref(),
            self.engineering_units
                .as_ref()
                .map(|units| units.display_name.as_str()),
            self.engineering_units
                .as_ref()
                .and_then(|units| units.description.as_deref()),
            self.two_state
                .as_ref()
                .map(|labels| labels.true_state.as_str()),
            self.two_state
                .as_ref()
                .map(|labels| labels.false_state.as_str()),
        ];
        if texts
            .into_iter()
            .flatten()
            .any(|text| text.len() > MAX_STRING_BYTES)
        {
            return Err(MetadataError::TextTooLong);
        }
        if let Some(units) = &self.engineering_units {
            if !numeric {
                return Err(MetadataError::UnitsOnNonNumeric(data_type));
            }
            if units.unit_id().is_none() {
                return Err(MetadataError::InvalidUneceCode);
            }
        }
        if let Some(range) = self.eu_range {
            if !numeric {
                return Err(MetadataError::RangeOnNonNumeric(data_type));
            }
            if !range.low.is_finite() || !range.high.is_finite() || range.low > range.high {
                return Err(MetadataError::InvalidRange);
            }
        }
        if self.two_state.is_some() && data_type != DataType::Boolean {
            return Err(MetadataError::TwoStateOnNonBoolean(data_type));
        }
        Ok(())
    }

    /// この属性を付けたノードの形（型定義と、持つプロパティ）。
    pub fn shape(&self, data_type: DataType) -> MetadataShape {
        let variable_type = if self.eu_range.is_some() {
            VariableType::AnalogItem
        } else if self.engineering_units.is_some() {
            VariableType::BaseAnalog
        } else if self.two_state.is_some() && data_type == DataType::Boolean {
            VariableType::TwoStateDiscrete
        } else {
            VariableType::BaseDataVariable
        };
        MetadataShape {
            variable_type,
            engineering_units: self.engineering_units.is_some(),
            eu_range: self.eu_range.is_some(),
            two_state: self.two_state.is_some(),
        }
    }
}

/// 変数ノードの型定義（HasTypeDefinition の先）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VariableType {
    /// BaseDataVariableType。属性の無いタグと String のタグ。
    BaseDataVariable,
    /// BaseAnalogType。単位だけを持つ数値のタグ。
    BaseAnalog,
    /// AnalogItemType。EURange を持つ数値のタグ（EURange が必須の型）。
    AnalogItem,
    /// TwoStateDiscreteType。文言を持つ Boolean のタグ（TrueState・FalseState が必須の型）。
    TwoStateDiscrete,
}

/// ノードの形。形が同じなら属性は同じ NodeId のまま書き換えられる。違えば
/// タグを削除して登録し直す（ADR-0008）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub struct MetadataShape {
    /// 型定義。
    pub variable_type: VariableType,
    /// EngineeringUnits プロパティを持つか。
    pub engineering_units: bool,
    /// EURange プロパティを持つか。
    pub eu_range: bool,
    /// TrueState・FalseState プロパティを持つか。
    pub two_state: bool,
}

/// タグに付けられない属性。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum MetadataError {
    /// 数値でないタグに単位を付けた。
    UnitsOnNonNumeric(DataType),
    /// 数値でないタグにレンジを付けた。
    RangeOnNonNumeric(DataType),
    /// Boolean でないタグに二値の文言を付けた。
    TwoStateOnNonBoolean(DataType),
    /// UNECE のコードが 1〜3 文字の英大文字・数字でない。
    InvalidUneceCode,
    /// レンジが非有限、または下限が上限を超える。
    InvalidRange,
    /// 文字列が [`MAX_STRING_BYTES`] を超える。
    TextTooLong,
}

impl std::fmt::Display for MetadataError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnitsOnNonNumeric(data_type) => {
                write!(f, "{data_type} のタグには単位を付けられません")
            }
            Self::RangeOnNonNumeric(data_type) => {
                write!(f, "{data_type} のタグにはレンジを付けられません")
            }
            Self::TwoStateOnNonBoolean(data_type) => {
                write!(f, "{data_type} のタグには二値の文言を付けられません")
            }
            Self::InvalidUneceCode => {
                f.write_str("UNECE のコードは 1〜3 文字の英大文字・数字にしてください")
            }
            Self::InvalidRange => f.write_str("レンジは有限で、下限を上限以下にしてください"),
            Self::TextTooLong => write!(f, "文字列は {MAX_STRING_BYTES} バイト以下にしてください"),
        }
    }
}

impl std::error::Error for MetadataError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_id_packs_the_unece_common_code() {
        // OPC 10000-8 の例: CEL（℃）は 4408652、KPA は 4935745。
        assert_eq!(
            EngineeringUnits::display("℃")
                .with_unece_code("CEL")
                .unit_id(),
            Some(4_408_652)
        );
        assert_eq!(
            EngineeringUnits::display("kPa")
                .with_unece_code("KPA")
                .unit_id(),
            Some(4_935_745)
        );
        assert_eq!(EngineeringUnits::display("個").unit_id(), Some(-1));
        for code in ["", "ABCD", "cel", "℃"] {
            assert_eq!(
                EngineeringUnits::display("x")
                    .with_unece_code(code)
                    .unit_id(),
                None,
                "{code}"
            );
        }
    }

    #[test]
    fn numeric_tags_take_units_and_range() {
        let metadata = TagMetadata::new()
            .with_description("炉内温度")
            .with_engineering_units(EngineeringUnits::display("℃").with_unece_code("CEL"))
            .with_eu_range(EuRange::new(0.0, 1200.0));
        for data_type in [
            DataType::Int16,
            DataType::UInt64,
            DataType::Float,
            DataType::Double,
        ] {
            assert_eq!(metadata.validate(data_type), Ok(()), "{data_type}");
        }
        assert_eq!(
            metadata.validate(DataType::Boolean),
            Err(MetadataError::UnitsOnNonNumeric(DataType::Boolean))
        );
        assert_eq!(
            TagMetadata::new()
                .with_eu_range(EuRange::new(0.0, 1.0))
                .validate(DataType::String),
            Err(MetadataError::RangeOnNonNumeric(DataType::String))
        );
    }

    #[test]
    fn two_state_labels_are_only_for_boolean() {
        let metadata = TagMetadata::new().with_two_state(TwoStateLabels::new("運転", "停止"));
        assert_eq!(metadata.validate(DataType::Boolean), Ok(()));
        assert_eq!(
            metadata.validate(DataType::Int16),
            Err(MetadataError::TwoStateOnNonBoolean(DataType::Int16))
        );
    }

    #[test]
    fn invalid_ranges_codes_and_long_texts_are_rejected() {
        for range in [
            EuRange::new(1.0, 0.0),
            EuRange::new(f64::NAN, 1.0),
            EuRange::new(0.0, f64::INFINITY),
        ] {
            assert_eq!(
                TagMetadata::new()
                    .with_eu_range(range)
                    .validate(DataType::Double),
                Err(MetadataError::InvalidRange),
                "{range:?}"
            );
        }
        assert_eq!(
            TagMetadata::new()
                .with_eu_range(EuRange::new(5.0, 5.0))
                .validate(DataType::Double),
            Ok(())
        );
        assert_eq!(
            TagMetadata::new()
                .with_engineering_units(EngineeringUnits::display("x").with_unece_code("cel"))
                .validate(DataType::Double),
            Err(MetadataError::InvalidUneceCode)
        );
        assert_eq!(
            TagMetadata::new()
                .with_description("a".repeat(MAX_STRING_BYTES + 1))
                .validate(DataType::Double),
            Err(MetadataError::TextTooLong)
        );
    }

    #[test]
    fn shape_selects_the_type_definition() {
        let units = EngineeringUnits::display("℃");
        let range = EuRange::new(0.0, 100.0);
        let cases = [
            (
                TagMetadata::new(),
                DataType::Double,
                VariableType::BaseDataVariable,
            ),
            (
                TagMetadata::new().with_description("説明だけ"),
                DataType::Int32,
                VariableType::BaseDataVariable,
            ),
            (
                TagMetadata::new().with_engineering_units(units.clone()),
                DataType::Double,
                VariableType::BaseAnalog,
            ),
            (
                TagMetadata::new().with_eu_range(range),
                DataType::Double,
                VariableType::AnalogItem,
            ),
            (
                TagMetadata::new()
                    .with_engineering_units(units)
                    .with_eu_range(range),
                DataType::Float,
                VariableType::AnalogItem,
            ),
            (
                TagMetadata::new().with_two_state(TwoStateLabels::new("開", "閉")),
                DataType::Boolean,
                VariableType::TwoStateDiscrete,
            ),
        ];
        for (metadata, data_type, expected) in cases {
            assert_eq!(
                metadata.shape(data_type).variable_type,
                expected,
                "{metadata:?}"
            );
        }
    }

    #[test]
    fn changing_only_texts_and_values_keeps_the_shape() {
        let before = TagMetadata::new()
            .with_description("旧")
            .with_engineering_units(EngineeringUnits::display("℃"))
            .with_eu_range(EuRange::new(0.0, 100.0));
        let after = TagMetadata::new()
            .with_engineering_units(EngineeringUnits::display("K").with_unece_code("KEL"))
            .with_eu_range(EuRange::new(273.0, 373.0));
        assert_eq!(
            before.shape(DataType::Double),
            after.shape(DataType::Double),
            "Description の有無と単位・レンジの値は形を変えない"
        );
        let without_range =
            TagMetadata::new().with_engineering_units(EngineeringUnits::display("℃"));
        assert_ne!(
            before.shape(DataType::Double),
            without_range.shape(DataType::Double)
        );
    }
}
