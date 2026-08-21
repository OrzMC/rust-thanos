use crate::mca::entry::McaEntry;
use crate::patterns::ChunkPattern;
use anyhow::Result;

pub struct ListPattern {
    coords: Vec<(i32, i32)>,
}

impl ListPattern {
    pub fn new(coords: Vec<(i32, i32)>) -> Self {
        Self { coords }
    }
}

impl ChunkPattern for ListPattern {
    fn matches(&self, entry: &mut McaEntry) -> Result<bool> {
        Ok(self
            .coords
            .iter()
            .any(|(x, z)| *x == entry.global_x() && *z == entry.global_z()))
    }

    fn box_clone(&self) -> Box<dyn ChunkPattern> {
        Box::new(Self {
            coords: self.coords.clone(),
        })
    }
}
