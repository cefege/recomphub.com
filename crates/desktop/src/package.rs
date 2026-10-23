use std::collections::HashMap;
use std::path::{Path, PathBuf};
use unicode_casefold::UnicodeCaseFold;
use unicode_normalization::UnicodeNormalization;

use recomphub_shared::validate_package_path_component;

use crate::DesktopError;

/// Validates a package-relative path using Windows rules on every host OS.
pub(crate) fn safe_zip_path(value: &str) -> Result<PathBuf, DesktopError> {
    if value.is_empty()
        || value.starts_with('/')
        || value.starts_with('\\')
        || value.contains('\\')
        || value.contains(':')
    {
        return Err(DesktopError::Package(
            "archive path is absolute or contains an unsafe separator".into(),
        ));
    }
    let mut path = PathBuf::new();
    for component in value.split('/') {
        validate_package_path_component("archive path", component).map_err(|_| {
            DesktopError::Package("archive path contains an unsafe component".into())
        })?;
        path.push(component);
    }
    Ok(path)
}

pub(crate) fn archive_collision_key(path: &Path) -> String {
    let mut key = String::new();
    for component in path.components() {
        if !key.is_empty() {
            key.push('/');
        }
        key.extend(component.as_os_str().to_string_lossy().case_fold().nfd());
    }
    key
}

pub(crate) struct ArchiveNode {
    original: PathBuf,
    directory: bool,
    explicit: bool,
}

pub(crate) fn insert_archive_entry(
    nodes: &mut HashMap<String, ArchiveNode>,
    path: &Path,
    directory: bool,
) -> Result<(), DesktopError> {
    let mut prefix = PathBuf::new();
    let components = path.components().count();
    for (index, component) in path.components().enumerate() {
        prefix.push(component);
        let leaf = index + 1 == components;
        let key = archive_collision_key(&prefix);
        match nodes.get_mut(&key) {
            Some(node) => {
                if node.original != prefix {
                    return Err(DesktopError::Package(
                        "archive contains case/normalization aliases in a path component".into(),
                    ));
                }
                if leaf {
                    if node.explicit || !directory {
                        return Err(DesktopError::Package(
                            "archive contains a duplicate or file/directory prefix collision"
                                .into(),
                        ));
                    }
                    node.explicit = true;
                } else if !node.directory {
                    return Err(DesktopError::Package(
                        "archive contains a file or link used as a parent directory".into(),
                    ));
                }
            }
            None => {
                nodes.insert(
                    key,
                    ArchiveNode {
                        original: prefix.clone(),
                        directory: !leaf || directory,
                        explicit: leaf,
                    },
                );
            }
        }
    }
    Ok(())
}

/// Resolves each original link component against the preflight tree, without writes.
pub(crate) fn resolve_archive_link(
    link: &Path,
    text: &str,
    nodes: &HashMap<String, ArchiveNode>,
) -> Result<PathBuf, DesktopError> {
    if text.is_empty()
        || text.len() > 4096
        || text.starts_with('/')
        || text.contains('\\')
        || text.contains(':')
        || text.chars().any(char::is_control)
    {
        return Err(DesktopError::Package(
            "archive symlink target is invalid".into(),
        ));
    }
    let app_root = link
        .ancestors()
        .find(|path| {
            path.extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("app"))
        })
        .ok_or_else(|| DesktopError::Package("macOS symlink is outside an app bundle".into()))?;
    let mut resolved = link
        .parent()
        .ok_or_else(|| DesktopError::Package("archive symlink has no parent".into()))?
        .to_path_buf();
    let mut components = text.split('/').peekable();
    while let Some(component) = components.next() {
        match component {
            "" => {
                return Err(DesktopError::Package(
                    "archive symlink target has an empty component".into(),
                ));
            }
            "." => {}
            ".." if !resolved.pop() => {
                return Err(DesktopError::Package(
                    "archive symlink escapes its package".into(),
                ));
            }
            ".." => {}
            value => {
                validate_package_path_component("archive symlink target", value).map_err(|_| {
                    DesktopError::Package("archive symlink target is invalid".into())
                })?;
                resolved.push(value);
            }
        }
        if !resolved.starts_with(app_root) {
            return Err(DesktopError::Package(
                "archive symlink escapes its app bundle".into(),
            ));
        }
        if components.peek().is_some() {
            let node = nodes
                .get(&archive_collision_key(&resolved))
                .ok_or_else(|| {
                    DesktopError::Package("archive symlink traverses a missing path".into())
                })?;
            if !node.directory {
                return Err(DesktopError::Package(
                    "archive symlink traverses a non-directory".into(),
                ));
            }
        }
    }
    Ok(resolved)
}

pub(crate) fn validate_archive_links(
    links: &HashMap<PathBuf, String>,
    files: &std::collections::HashSet<PathBuf>,
    nodes: &HashMap<String, ArchiveNode>,
) -> Result<(), DesktopError> {
    let mut resolved_links = std::collections::HashSet::new();
    for start in links.keys() {
        if resolved_links.contains(start) {
            continue;
        }
        let mut current = start.clone();
        let mut chain = std::collections::HashSet::new();
        loop {
            if files.contains(&current) || resolved_links.contains(&current) {
                resolved_links.extend(chain);
                break;
            }
            if !chain.insert(current.clone()) {
                return Err(DesktopError::Package("archive symlink cycle".into()));
            }
            let text = links.get(&current).ok_or_else(|| {
                DesktopError::Package(
                    "archive symlink target is not an extracted regular file".into(),
                )
            })?;
            current = resolve_archive_link(&current, text, nodes)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn full_casefold_detects_sigma_aliases() {
        assert_eq!(
            archive_collision_key(Path::new("Σ")),
            archive_collision_key(Path::new("ς")),
        );
    }

    #[test]
    fn shared_windows_path_component_rules_apply_to_archive_paths() {
        for value in [
            "a*b",
            "a?b",
            "a\"b",
            "a<b",
            "a>b",
            "a|b",
            "COM¹.txt",
            "LPT²",
        ] {
            assert!(safe_zip_path(value).is_err(), "{value:?}");
        }
    }

    #[test]
    fn symlink_traversal_checks_original_components_and_tree_types() {
        let link = Path::new("Game.app/Contents/Frameworks/link.dylib");
        let mut nodes = HashMap::new();
        for path in [
            "Game.app",
            "Game.app/Contents",
            "Game.app/Contents/Frameworks",
            "Game.app/Contents/Resources",
        ] {
            insert_archive_entry(&mut nodes, Path::new(path), true).unwrap();
        }
        insert_archive_entry(
            &mut nodes,
            Path::new("Game.app/Contents/Resources/real.dylib"),
            false,
        )
        .unwrap();

        assert!(resolve_archive_link(link, "missing/../Resources/real.dylib", &nodes).is_err());
        assert!(resolve_archive_link(link, "Resources/real.dylib/", &nodes).is_err());

        let mut leaf_nodes = nodes;
        insert_archive_entry(
            &mut leaf_nodes,
            Path::new("Game.app/Contents/Frameworks/not-a-dir"),
            false,
        )
        .unwrap();
        assert!(resolve_archive_link(link, "not-a-dir/real.dylib", &leaf_nodes).is_err());
    }

    #[test]
    fn same_app_link_chains_resolve_to_regular_files() {
        let mut nodes = HashMap::new();
        for path in [
            "Game.app",
            "Game.app/Contents",
            "Game.app/Contents/Frameworks",
        ] {
            insert_archive_entry(&mut nodes, Path::new(path), true).unwrap();
        }
        let first = PathBuf::from("Game.app/Contents/Frameworks/first.dylib");
        let second = PathBuf::from("Game.app/Contents/Frameworks/second.dylib");
        let real = PathBuf::from("Game.app/Contents/Frameworks/real.dylib");
        insert_archive_entry(&mut nodes, &first, false).unwrap();
        insert_archive_entry(&mut nodes, &second, false).unwrap();
        insert_archive_entry(&mut nodes, &real, false).unwrap();
        let links = HashMap::from([
            (first.clone(), "second.dylib".to_owned()),
            (second, "real.dylib".to_owned()),
        ]);
        let files = std::collections::HashSet::from([real]);
        assert!(validate_archive_links(&links, &files, &nodes).is_ok());
    }

    #[test]
    fn cross_platform_path_rules_reject_aliases_and_devices() {
        for value in [
            "../x",
            "a/../x",
            "a\\x",
            "C:/x",
            "CON.txt",
            "dir/NUL.log",
            "name.",
            "name ",
            "a//b",
            "a\u{0001}b",
        ] {
            assert!(safe_zip_path(value).is_err(), "{value:?}");
        }
        assert_eq!(
            safe_zip_path("BanjoRecompiled.app/Contents/MacOS/Game").unwrap(),
            PathBuf::from("BanjoRecompiled.app/Contents/MacOS/Game")
        );
    }

    #[test]
    fn decomposed_and_composed_unicode_paths_alias() {
        assert_eq!(
            archive_collision_key(Path::new("caf\u{00e9}/Game")),
            archive_collision_key(Path::new("cafe\u{0301}/Game"))
        );
    }

    #[test]
    fn archive_preflight_rejects_unicode_normalization_aliases() {
        let mut entries = HashMap::new();
        insert_archive_entry(&mut entries, Path::new("café/file"), false).unwrap();
        assert!(
            insert_archive_entry(&mut entries, Path::new("cafe\u{0301}/other"), false).is_err()
        );
    }
}
