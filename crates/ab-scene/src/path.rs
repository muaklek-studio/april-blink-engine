use std::fmt;

/// Path to a node, Godot style.
///
/// * `"Child/GrandChild"`: relative to the starting node.
/// * `"../Sibling"`: `..` moves to the parent, `.` stays.
/// * `"/root/Level/Player"`: absolute, starting at the tree root (named `root`).
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct NodePath(String);

/// One step of a [`NodePath`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathSegment<'a> {
    Current,
    Parent,
    Name(&'a str),
}

impl NodePath {
    pub fn new(path: impl Into<String>) -> Self {
        Self(path.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn is_absolute(&self) -> bool {
        is_absolute(&self.0)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn segments(&self) -> impl Iterator<Item = PathSegment<'_>> {
        segments(&self.0)
    }
}

pub(crate) fn is_absolute(path: &str) -> bool {
    path.starts_with('/')
}

pub(crate) fn segments(path: &str) -> impl Iterator<Item = PathSegment<'_>> {
    path.split('/').filter(|s| !s.is_empty()).map(|s| match s {
        "." => PathSegment::Current,
        ".." => PathSegment::Parent,
        name => PathSegment::Name(name),
    })
}

impl fmt::Debug for NodePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "NodePath({:?})", self.0)
    }
}

impl fmt::Display for NodePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<&str> for NodePath {
    fn from(s: &str) -> Self {
        Self::new(s)
    }
}

impl From<String> for NodePath {
    fn from(s: String) -> Self {
        Self::new(s)
    }
}

impl AsRef<str> for NodePath {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_segments() {
        let path = NodePath::new("../A/./B/");
        assert!(!path.is_absolute());
        let segs: Vec<_> = path.segments().collect();
        assert_eq!(
            segs,
            [
                PathSegment::Parent,
                PathSegment::Name("A"),
                PathSegment::Current,
                PathSegment::Name("B")
            ]
        );
        assert!(NodePath::from("/root/A").is_absolute());
    }
}
