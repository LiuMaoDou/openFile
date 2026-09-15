//! Coalesce file/subtree events; retain full recovery when native events are lost.
use std::{collections::BTreeSet, path::PathBuf};

#[derive(Default)]
pub(crate) struct Pending {
    pub full: bool,
    pub reason: Option<&'static str>,
    pub paths: BTreeSet<PathBuf>,
}
impl Pending {
    pub fn rescan(&mut self) {
        self.rescan_because("手动刷新或启动核对");
    }
    pub fn rescan_because(&mut self, reason: &'static str) {
        self.full = true;
        self.reason = Some(reason);
        self.paths.clear();
    }
    pub fn add(&mut self, paths: impl IntoIterator<Item = PathBuf>) {
        if self.full {
            return;
        }
        for path in paths {
            if path.ancestors().any(|parent| self.paths.contains(parent)) {
                continue;
            }
            let descendants: Vec<_> = self
                .paths
                .range(path.clone()..)
                .take_while(|child| child.starts_with(&path))
                .cloned()
                .collect();
            for child in descendants {
                self.paths.remove(&child);
            }
            self.paths.insert(path);
            if self.paths.len() > 8192 {
                self.rescan_because("积累的文件变化超过 8192 项，需要完整核对");
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parent_events_subsume_children_but_not_similar_siblings() {
        let mut pending = Pending::default();
        pending.add(
            [
                "root/child/file",
                "root/child/deep/file",
                "root/child-other/file",
            ]
            .map(PathBuf::from),
        );
        pending.add([PathBuf::from("root/child")]);
        pending.add([PathBuf::from("root/child/new")]);
        assert_eq!(
            pending.paths,
            ["root/child", "root/child-other/file"]
                .into_iter()
                .map(PathBuf::from)
                .collect()
        );
    }
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
