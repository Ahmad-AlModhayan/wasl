//! What an on-demand folder shows: every file and folder of the pair with its size and time,
//! whether or not its content is here. Built from the pair's listing; the back ends turn it into
//! placeholders (Windows), inodes (FUSE) or items (File Provider, DocumentsProvider).

use std::collections::BTreeMap;

use crate::FileRef;

/// One file or folder of an on-demand folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    /// `/`-separated, relative to the folder; never empty, never `.`/`..` components.
    pub path: String,
    pub is_dir: bool,
    /// Milliseconds since the Unix epoch.
    pub mtime_ms: i64,
    /// The content to read (files only).
    pub file: Option<FileRef>,
}

impl Node {
    pub fn size(&self) -> u64 {
        self.file.as_ref().map_or(0, |f| f.size)
    }

    pub fn name(&self) -> &str {
        self.path.rsplit_once('/').map_or(self.path.as_str(), |(_, n)| n)
    }

    pub fn parent(&self) -> &str {
        self.path.rsplit_once('/').map_or("", |(p, _)| p)
    }
}

/// Every node of an on-demand folder, by path, with each folder's children.
#[derive(Debug, Clone, Default)]
pub struct Tree {
    nodes: BTreeMap<String, Node>,
}

impl Tree {
    /// Builds the tree; nodes with unsafe paths are dropped, and missing parent folders are added.
    pub fn new(nodes: impl IntoIterator<Item = Node>) -> Self {
        let mut map = BTreeMap::new();
        for n in nodes {
            if !safe_path(&n.path) || (!n.is_dir && n.file.is_none()) {
                continue;
            }
            let mut at = 0;
            while let Some(i) = n.path[at..].find('/') {
                let dir = n.path[..at + i].to_owned();
                map.entry(dir.clone()).or_insert(Node {
                    path: dir,
                    is_dir: true,
                    mtime_ms: n.mtime_ms,
                    file: None,
                });
                at += i + 1;
            }
            map.insert(n.path.clone(), n);
        }
        Self { nodes: map }
    }

    pub fn get(&self, path: &str) -> Option<&Node> {
        self.nodes.get(path)
    }

    /// The children of a folder (`""` is the root), in name order.
    pub fn children<'a>(&'a self, dir: &'a str) -> impl Iterator<Item = &'a Node> + 'a {
        self.nodes.values().filter(move |n| n.parent() == dir)
    }

    pub fn iter(&self) -> impl Iterator<Item = &Node> {
        self.nodes.values()
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }
}

/// A path an on-demand folder may show: the same check every local path in Wasl goes through
/// (`/`-separated, no empty, `.` or `..` component, no NUL or backslash, not absolute, and on
/// Windows no drive prefix, colon, trailing dot or space, device name or 8.3 short name), and not
/// empty.
pub fn safe_path(p: &str) -> bool {
    !p.is_empty() && wasl_fs::check_rel(p).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str) -> Node {
        Node {
            path: path.into(),
            is_dir: false,
            mtime_ms: 1,
            file: Some(FileRef {
                id: path.into(),
                version: "1".into(),
                size: 3,
            }),
        }
    }

    #[test]
    fn folders_are_filled_in_and_unsafe_paths_dropped() {
        let t = Tree::new([
            file("a/b/c.txt"),
            file("top.txt"),
            file("../x"),
            file("a//y"),
            file(r"a\z"),
            file("C:/evil"),
            file("a/CON"),
            file("a/b:c"),
        ]);
        assert!(t.get("a").unwrap().is_dir);
        assert!(t.get("a/b").unwrap().is_dir);
        assert_eq!(
            vec!["a", "top.txt"],
            t.children("").map(|n| n.path.as_str()).collect::<Vec<_>>()
        );
        assert_eq!(
            vec!["a/b/c.txt"],
            t.children("a/b").map(|n| n.path.as_str()).collect::<Vec<_>>()
        );
        // A drive prefix is refused everywhere; device names and colons only where Windows reads them.
        assert!(t.get("C:/evil").is_none() && t.get("C:").is_none());
        assert_eq!(if cfg!(windows) { 4 } else { 6 }, t.len());
        assert_eq!("c.txt", t.get("a/b/c.txt").unwrap().name());
    }
}
