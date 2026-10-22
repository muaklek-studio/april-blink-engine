/// Normalises a virtual path: `\` → `/`, drops `.` and empty segments, resolves `..`.
/// Returns `None` for empty paths and paths escaping the root.
pub fn normalize(path: &str) -> Option<String> {
    let mut parts: Vec<&str> = Vec::new();
    for part in path.split(['/', '\\']) {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            name => parts.push(name),
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes() {
        assert_eq!(normalize("a\\b/./c").as_deref(), Some("a/b/c"));
        assert_eq!(normalize("/a/b/../c").as_deref(), Some("a/c"));
        assert_eq!(normalize("../secret"), None);
        assert_eq!(normalize("a/../.."), None);
        assert_eq!(normalize(""), None);
        assert_eq!(normalize("./"), None);
    }
}
