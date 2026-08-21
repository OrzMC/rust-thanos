pub mod inhabited;
pub mod list;
pub mod range;

use crate::mca::entry::McaEntry;

pub trait ChunkPattern: Send + Sync {
    fn matches(&self, entry: &mut McaEntry) -> anyhow::Result<bool>;
    /// Clone the pattern into a new boxed trait object. Needed so patterns can be
    /// cloned into each parallel dimension-processing task.
    fn box_clone(&self) -> Box<dyn ChunkPattern>;
}

impl Clone for Box<dyn ChunkPattern> {
    fn clone(&self) -> Self {
        self.box_clone()
    }
}
