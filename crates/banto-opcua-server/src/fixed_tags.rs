//! Phase 1 の実接続試験に使う固定タグ。ハブのタグレジストリとの結合は Phase 2。

use std::sync::Arc;

use opcua::{
    server::{
        ServerHandle,
        address_space::{ObjectBuilder, Variable},
        diagnostics::NamespaceMetadata,
        node_manager::memory::{SimpleNodeManager, simple_node_manager},
    },
    types::{DataValue, NodeId, ObjectId, QualifiedName, ReferenceTypeId, VariableTypeId},
};

use crate::Error;

const MANAGER_NAME: &str = "banto-fixed-tags";

/// Phase 1 の固定ノードを扱う node manager を作る。
pub(crate) fn node_manager(
    namespace_uri: String,
) -> impl opcua::server::node_manager::NodeManagerBuilder {
    simple_node_manager(
        NamespaceMetadata {
            namespace_uri,
            ..Default::default()
        },
        MANAGER_NAME,
    )
}

/// 固定タグのノード ID と値更新に必要なハンドル。
pub(crate) struct FixedTags {
    pub namespace_index: u16,
    manager: Arc<SimpleNodeManager>,
}

impl FixedTags {
    /// 起動前に固定階層を登録する。
    pub fn register(handle: &ServerHandle, namespace_uri: &str) -> Result<Self, Error> {
        let namespace_index = handle
            .get_namespace_index(namespace_uri)
            .ok_or_else(|| Error::Build("固定タグの名前空間が登録されていません".to_owned()))?;
        let manager = handle
            .node_managers()
            .get_by_name::<SimpleNodeManager>(MANAGER_NAME)
            .ok_or_else(|| Error::Build("固定タグの node manager がありません".to_owned()))?;
        let banto = NodeId::new(namespace_index, "Banto");
        let plc = NodeId::new(namespace_index, "PLC1");
        let line = NodeId::new(namespace_index, "PLC1.Line1");
        {
            let mut space = manager.address_space().write();
            for (id, name, parent) in [
                (&banto, "Banto", ObjectId::ObjectsFolder.into()),
                (&plc, "PLC1", banto.clone()),
                (&line, "Line1", plc.clone()),
            ] {
                let inserted =
                    ObjectBuilder::new(id, QualifiedName::new(namespace_index, name), name)
                        .is_folder()
                        .organized_by(parent)
                        .insert(&mut *space);
                if !inserted {
                    return Err(Error::Build(format!(
                        "固定フォルダー {name} を登録できません"
                    )));
                }
            }

            for (name, value) in [
                ("Speed", opcua::types::Variant::Double(0.0)),
                ("Running", opcua::types::Variant::Boolean(false)),
            ] {
                let id = NodeId::new(namespace_index, format!("PLC1.Line1.{name}"));
                let inserted = space.insert::<_, NodeId>(
                    Variable::new(&id, QualifiedName::new(namespace_index, name), name, value),
                    None,
                );
                if !inserted {
                    return Err(Error::Build(format!("固定タグ {name} を登録できません")));
                }
                space.insert_reference(&line, &id, ReferenceTypeId::HasComponent);
                space.insert_reference(
                    &id,
                    &VariableTypeId::BaseDataVariableType.into(),
                    ReferenceTypeId::HasTypeDefinition,
                );
            }
        }
        Ok(Self {
            namespace_index,
            manager,
        })
    }

    pub fn update(&self, handle: &ServerHandle, name: &str, value: DataValue) -> Result<(), Error> {
        let id = NodeId::new(self.namespace_index, format!("PLC1.Line1.{name}"));
        self.manager
            .set_value(handle.subscriptions(), &id, None, value)
            .map_err(|status| Error::TagUpdate(status.to_string()))
    }
}
