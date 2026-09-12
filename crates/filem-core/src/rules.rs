use anyhow::{bail, Result};
use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use std::path::Path;

pub struct Excludes {
    bare: GlobSet,
    relative: GlobSet,
}
impl Excludes {
    pub fn new(patterns: &[String]) -> Result<Self> {
        if patterns.len() > 128 {
            bail!("排除规则最多 128 条。");
        }
        let mut bare = GlobSetBuilder::new();
        let mut relative = GlobSetBuilder::new();
        for raw in patterns {
            let p = raw.trim().replace('\\', "/");
            if p.is_empty() {
                continue;
            }
            if p.starts_with('!') || p.starts_with('/') || p.split('/').any(|s| s == "..") {
                bail!("排除规则必须是相对路径，不支持 ! 或 ..：{raw}");
            }
            let g = GlobBuilder::new(p.trim_end_matches('/'))
                .literal_separator(true)
                .build()?;
            if p.contains('/') {
                relative.add(g);
            } else {
                bare.add(g);
            }
        }
        Ok(Self {
            bare: bare.build()?,
            relative: relative.build()?,
        })
    }
    pub fn matches(&self, relative: &Path) -> bool {
        relative
            .ancestors()
            .any(|path| self.relative.is_match(path))
            || relative
                .components()
                .any(|p| self.bare.is_match(Path::new(p.as_os_str())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn relative_directory_exclusions_cover_descendants_without_prefix_overreach() {
        let rules = Excludes::new(&[
            "private/archive".into(),
            "node_modules".into(),
            "*.tmp".into(),
        ])
        .unwrap();
        for path in [
            "private/archive",
            "private/archive/a.txt",
            "private/archive/deep/a.txt",
            "x/node_modules/a.js",
            "a.tmp",
        ] {
            assert!(rules.matches(Path::new(path)), "{path}");
        }
        for path in [
            "private/archives/a.txt",
            "other/private/archive/a.txt",
            "private/readme.txt",
            "a.tmp.md",
        ] {
            assert!(!rules.matches(Path::new(path)), "{path}");
        }
    }
}
