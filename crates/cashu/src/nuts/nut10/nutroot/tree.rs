use super::{branch, Error, Leaf};

/// A canonical Nutroot tree containing one to eight validated leaves.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tree(Vec<Leaf>);

impl Tree {
    /// Build a tree, preserving transmitted leaf order for slot assignment.
    pub fn new(leaves: Vec<Leaf>) -> Result<Self, Error> {
        if leaves.is_empty() || leaves.len() > 8 {
            return Err(Error::InvalidTree);
        }
        Ok(Self(leaves))
    }

    /// Leaves in transmitted order.
    pub fn leaves(&self) -> &[Leaf] {
        &self.0
    }

    /// Root from the sorted, pairwise fold, with odd nodes promoted unchanged.
    pub fn root(&self) -> [u8; 32] {
        self.fold(None).0
    }

    /// Sibling hashes for a leaf's transmitted index.
    pub fn path(&self, index: usize) -> Result<Vec<[u8; 32]>, Error> {
        if index >= self.0.len() {
            return Err(Error::InvalidTree);
        }
        Ok(self.fold(Some(index)).1)
    }

    fn fold(&self, index: Option<usize>) -> ([u8; 32], Vec<[u8; 32]>) {
        let mut nodes: Vec<_> = self
            .0
            .iter()
            .enumerate()
            .map(|(i, leaf)| (leaf.hash(), Some(i) == index))
            .collect();
        nodes.sort_by_key(|(hash, _)| *hash);
        let mut path = vec![];
        while nodes.len() > 1 {
            nodes = nodes
                .chunks(2)
                .map(|pair| {
                    if pair.len() == 1 {
                        return pair[0];
                    }
                    let (a, b) = (pair[0], pair[1]);
                    if a.1 {
                        path.push(b.0);
                    }
                    if b.1 {
                        path.push(a.0);
                    }
                    (branch(a.0, b.0), a.1 || b.1)
                })
                .collect();
        }
        (nodes[0].0, path)
    }
}
