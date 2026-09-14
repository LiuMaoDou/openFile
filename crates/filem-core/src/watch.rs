//! Coalesce ordinary file events; retain full recovery for directory/overflow events.
use std::{collections::BTreeSet, path::PathBuf};

#[derive(Default)]
pub(crate) struct Pending {
    pub full: bool,
    pub paths: BTreeSet<PathBuf>,
}
impl Pending {
    pub fn rescan(&mut self) {
        self.full = true;
        self.paths.clear();
    }
    pub fn add(&mut self, paths: impl IntoIterator<Item = PathBuf>) {
        if self.full {
            return;
        }
        for path in paths {
            self.paths.insert(path);
            if self.paths.len() > 8192 {
                self.rescan();
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn events_are_deduplicated_and_overflow_requires_full_recovery() {
        let mut pending = Pending::default();
        pending.add(vec![PathBuf::from("same"); 2]);
        assert_eq!(pending.paths.len(), 1);
        pending.add((0..8193).map(|i| PathBuf::from(i.to_string())));
        assert!(pending.full);
        assert!(pending.paths.is_empty());
        pending.add([PathBuf::from("later")]);
        assert!(pending.paths.is_empty());
    }
}
