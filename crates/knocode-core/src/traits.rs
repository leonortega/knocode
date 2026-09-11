use crate::error::Result;
use crate::ipc::{ContextPack, TaskRequest};

/// IContextBuilder — in-process | daemon | remote (spec §2 portability, ARCHITECTURE.md:209-241)
/// Reference implementation: Rust daemon (`ContextEngine`) via UDS+MessagePack.
/// Swappable behind this trait; concrete reason for Rust choice remains embedded tree-sitter crates.
#[async_trait::async_trait]
pub trait IContextBuilder: Send + Sync {
    async fn build_context(&self, task: &TaskRequest) -> Result<ContextPack>;
    fn to_yaml(pack: &ContextPack) -> Result<String>
    where
        Self: Sized;
}
