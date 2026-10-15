//! アドレス空間に置くノードの形。識別子の永続化方針（外部名か安定 ID か）は
//! 未決なので、この型はどちらの文字列も数値も入れられるようにしてある。
//! 中身の方針は README の「オーナー判断待ち」。

use crate::DataType;

/// NodeId の識別子。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Identifier {
    /// 数値識別子（`ns=0;i=2259` の `2259`）。
    Numeric(u32),
    /// 文字列識別子（`ns=2;s=PLC1.Line1.Speed` の文字列部）。
    String(String),
}

/// OPC UA NodeId。名前空間インデックスと識別子の組。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NodeId {
    /// 名前空間インデックス。標準ノードは 0、banto のタグはサーバーが割り当てた値。
    pub namespace: u16,
    /// その名前空間の中の識別子。
    pub identifier: Identifier,
}

impl NodeId {
    /// 数値 NodeId を作る。
    pub const fn numeric(namespace: u16, id: u32) -> Self {
        Self {
            namespace,
            identifier: Identifier::Numeric(id),
        }
    }

    /// 文字列 NodeId を作る。
    pub fn string(namespace: u16, id: impl Into<String>) -> Self {
        Self {
            namespace,
            identifier: Identifier::String(id.into()),
        }
    }
}

impl std::fmt::Display for NodeId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.identifier {
            Identifier::Numeric(id) => write!(f, "ns={};i={id}", self.namespace),
            Identifier::String(id) => write!(f, "ns={};s={id}", self.namespace),
        }
    }
}

/// ノードのクラス。Phase 1 で使うのはオブジェクトと変数。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NodeClass {
    /// 階層を表すオブジェクト。
    Object,
    /// 値を持つ変数。
    Variable,
}

/// AccessLevel / UserAccessLevel のうち、この足場が区別するビット。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AccessLevel {
    /// CurrentRead。
    pub current_read: bool,
    /// CurrentWrite。Phase 1 では false のまま。
    pub current_write: bool,
}

impl AccessLevel {
    /// 読み取り専用。#201 の初期スコープ（書き込みは別 issue）。
    pub const fn current_read_only() -> Self {
        Self {
            current_read: true,
            current_write: false,
        }
    }
}

/// 公開する変数ノード。値そのものは持たない（値は取得のたびにハブから読む）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VariableNode {
    /// この変数の NodeId。
    pub node_id: NodeId,
    /// BrowseName。
    pub browse_name: String,
    /// DisplayName。
    pub display_name: String,
    /// 公開する DataType。
    pub data_type: DataType,
    /// アクセスレベル。Phase 1 は読み取り専用。
    pub access: AccessLevel,
    /// ノードクラス。変数として固定。
    pub node_class: NodeClass,
}

impl VariableNode {
    /// 読み取り専用の変数ノードを作る。
    pub fn read_only(
        node_id: NodeId,
        browse_name: impl Into<String>,
        display_name: impl Into<String>,
        data_type: DataType,
    ) -> Self {
        Self {
            node_id,
            browse_name: browse_name.into(),
            display_name: display_name.into(),
            data_type,
            access: AccessLevel::current_read_only(),
            node_class: NodeClass::Variable,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_matches_the_opc_ua_text_form() {
        assert_eq!(NodeId::numeric(0, 2259).to_string(), "ns=0;i=2259");
        assert_eq!(
            NodeId::string(2, "PLC1.Line1.Speed").to_string(),
            "ns=2;s=PLC1.Line1.Speed"
        );
    }

    #[test]
    fn phase1_variable_is_read_only() {
        let node = VariableNode::read_only(
            NodeId::string(2, "PLC1.Line1.Speed"),
            "Speed",
            "Speed",
            DataType::Double,
        );
        assert!(node.access.current_read);
        assert!(!node.access.current_write);
        assert_eq!(node.node_class, NodeClass::Variable);
    }
}
