//! タグを置く node manager（ADR-0004）。
//!
//! 読み取りと購読は async-opcua の `InMemoryNodeManagerImpl` の既定実装に任せ、
//! 書き込みだけを自前で扱う。Write はノードの値を書き換えず、検査を通った
//! ものだけを利用者の [`WriteHandler`] へ渡す。ハンドラーが無ければすべて
//! `BadNotWritable`。

use std::{collections::HashMap, num::NonZeroUsize, sync::Arc};

use async_trait::async_trait;
use opcua::{
    server::{
        address_space::{AccessLevel, AddressSpace, NodeType, user_access_level},
        diagnostics::NamespaceMetadata,
        node_manager::{
            NodeManagerBuilder, RequestContext, ServerContext, WriteNode,
            memory::{InMemoryNodeManager, InMemoryNodeManagerBuilder, InMemoryNodeManagerImpl},
        },
    },
    sync::RwLock,
    types::{AttributeId, DataTypeId, NodeId, NumericRange, StatusCode, Variant},
};
use tokio::sync::Semaphore;

use banto_opcua_core::{
    MappedValue, WriteClient, WritePrincipal, WriteRequest, status_code_for_write_rejection,
};

use crate::WriteHandler;

pub(crate) type TagNodeManager = InMemoryNodeManager<TagNodeManagerImpl>;

/// 動的タグの外部名。NodeId の文字列は名前のドットで曖昧になるので、登録時の
/// 名前をそのまま持つ。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TagName {
    pub connection: String,
    pub group: String,
    pub tag: String,
}

/// 書き込みの経路。ハンドラーと、同時に走るハンドラーの数の上限。
pub(crate) struct WritePath {
    pub handler: Arc<dyn WriteHandler>,
    pub permits: Arc<Semaphore>,
}

impl WritePath {
    pub fn new(handler: Arc<dyn WriteHandler>, max_concurrent: NonZeroUsize) -> Self {
        Self {
            handler,
            permits: Arc::new(Semaphore::new(max_concurrent.get())),
        }
    }
}

pub(crate) fn node_manager(
    namespace_uri: String,
    name: &'static str,
    write: Option<WritePath>,
) -> impl NodeManagerBuilder {
    InMemoryNodeManagerBuilder::new(
        move |context: ServerContext, address_space: &mut AddressSpace| {
            // SimpleNodeManager と同じく、型ツリーとアドレス空間の両方へ名前空間を登録する。
            let namespace_index = context
                .type_tree
                .write()
                .namespaces_mut()
                .add_namespace(&namespace_uri);
            address_space.add_namespace(&namespace_uri, namespace_index);
            TagNodeManagerImpl {
                namespaces: vec![NamespaceMetadata {
                    namespace_uri,
                    namespace_index,
                    ..Default::default()
                }],
                name,
                write,
                tags: RwLock::new(HashMap::new()),
            }
        },
    )
}

pub(crate) struct TagNodeManagerImpl {
    namespaces: Vec<NamespaceMetadata>,
    name: &'static str,
    write: Option<WritePath>,
    /// 動的タグの外部名。アドレス空間の write ロックの中で更新し、read ロックの
    /// 中で読む（ロックの順はアドレス空間が先）。
    tags: RwLock<HashMap<NodeId, TagName>>,
}

impl TagNodeManagerImpl {
    /// ハンドラーがあり、タグを書き込み可にできるか。
    pub fn accepts_writes(&self) -> bool {
        self.write.is_some()
    }

    pub fn tags(&self) -> &RwLock<HashMap<NodeId, TagName>> {
        &self.tags
    }
}

#[async_trait]
impl InMemoryNodeManagerImpl for TagNodeManagerImpl {
    async fn init(&self, _address_space: &mut AddressSpace, _context: ServerContext) {}

    fn name(&self) -> &str {
        self.name
    }

    fn namespaces(&self) -> Vec<NamespaceMetadata> {
        self.namespaces.clone()
    }

    async fn write(
        &self,
        context: &RequestContext,
        address_space: &RwLock<AddressSpace>,
        nodes_to_write: &mut [&mut WriteNode],
    ) -> Result<(), StatusCode> {
        let Some(write) = &self.write else {
            for node in nodes_to_write.iter_mut() {
                node.set_status(StatusCode::BadNotWritable);
            }
            return Ok(());
        };
        let client = write_client(context);
        for node in nodes_to_write.iter_mut() {
            // async-opcua は切断で要求を捨てないので、CloseSession の後は残りを始めない。
            if !context.session.read().is_activated() {
                node.set_status(StatusCode::BadSessionClosed);
                continue;
            }
            // 検査はロックの中で終え、ハンドラーを待つ間はロックを持たない。
            let checked = check_write(context, &address_space.read(), &self.tags.read(), node);
            let request = match checked {
                Ok((name, value)) => WriteRequest::new(
                    name.connection,
                    name.group,
                    name.tag,
                    value,
                    WritePrincipal::Anonymous,
                    client.clone(),
                ),
                Err(status) => {
                    node.set_status(status);
                    continue;
                }
            };
            // 上限では待たずに断る。待ち行列にすると応答待ちのタスクが積み上がる。
            let Ok(permit) = write.permits.clone().try_acquire_owned() else {
                node.set_status(StatusCode::BadResourceUnavailable);
                continue;
            };
            let handler = write.handler.clone();
            // 要求が捨てられても、始めた 1 件は最後まで走らせる。
            let result = tokio::spawn(async move {
                let _permit = permit;
                handler.write(request).await
            })
            .await;
            node.set_status(match result {
                Ok(Ok(())) => StatusCode::Good,
                Ok(Err(rejection)) => {
                    StatusCode::from(status_code_for_write_rejection(rejection).code())
                }
                Err(_) => StatusCode::BadInternalError,
            });
        }
        Ok(())
    }
}

fn write_client(context: &RequestContext) -> WriteClient {
    let session = context.session.read();
    let application = session.application_description();
    WriteClient::new(
        session.endpoint_url().as_ref(),
        session.session_id().to_string(),
        session.session_name(),
        application.application_uri.as_ref(),
        application.application_name.text.as_ref(),
    )
}

/// ADR-0004 の「サーバーが返すもの」の検査。
fn check_write(
    context: &RequestContext,
    space: &AddressSpace,
    tags: &HashMap<NodeId, TagName>,
    node: &WriteNode,
) -> Result<(TagName, MappedValue), StatusCode> {
    let request = node.value();
    let found = space
        .find(&request.node_id)
        .ok_or(StatusCode::BadNodeIdUnknown)?;
    let NodeType::Variable(variable) = found else {
        return Err(StatusCode::BadNotWritable);
    };
    if request.attribute_id != AttributeId::Value
        || !variable.access_level().contains(AccessLevel::CURRENT_WRITE)
    {
        return Err(StatusCode::BadNotWritable);
    }
    // 開いているのは Anonymous のエンドポイントだけ。ほかの主体は Phase 3 で足す。
    if !context.token.is_anonymous()
        || !user_access_level(context, found).contains(AccessLevel::CURRENT_WRITE)
    {
        return Err(StatusCode::BadUserAccessDenied);
    }
    if request.index_range != NumericRange::None {
        return Err(StatusCode::BadIndexRangeNoData);
    }
    if request.value.status.is_some_and(|status| !status.is_good()) {
        return Err(StatusCode::BadWriteNotSupported);
    }
    let data_type = variable.data_type();
    let value = match &request.value.value {
        Some(Variant::Boolean(bit)) if data_type == DataTypeId::Boolean => {
            MappedValue::Boolean(*bit)
        }
        Some(Variant::Double(number)) if data_type == DataTypeId::Double => {
            if !number.is_finite() {
                return Err(StatusCode::BadOutOfRange);
            }
            MappedValue::Double(*number)
        }
        _ => return Err(StatusCode::BadTypeMismatch),
    };
    // 書き込み可にできるのは動的タグだけなので、ここで外部名が無いのは同期のずれ。
    let name = tags
        .get(&request.node_id)
        .cloned()
        .ok_or(StatusCode::BadNotWritable)?;
    Ok((name, value))
}
