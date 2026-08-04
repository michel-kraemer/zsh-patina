use std::{
    fs::{self, Metadata},
    os::unix::fs::PermissionsExt,
    path::{Component, Path},
};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PathType {
    File,
    Directory,
}

/// Return the name from a Zsh named-directory expression such as `~name/path`.
pub fn named_directory(path: &str) -> Option<&str> {
    let name = path.strip_prefix('~')?.split('/').next()?;
    (!name.is_empty()
        && name
            .chars()
            .all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.')))
    .then_some(name)
}

/// Find a file or directory that starts with the given prefix and return its
/// metadata.
/// * If the prefix is relative, it is resolved against the provided `pwd`.
/// * If multiple files or directories match the prefix, the function returns
///   the first one returned by `read_dir`, which is not guaranteed to be in any
///   particular order.
/// * If the prefix does not match any file or directory, or if the user lacks
///   permission to access it, the function returns `None`.
fn find_by_prefix(prefix: &str, pwd: &str) -> Option<(Metadata, bool)> {
    let path = Path::new(prefix);
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        Path::new(pwd).join(path)
    };

    let mut comps = path.components();
    let last = comps.next_back()?;
    let (parent, name) = match last {
        Component::CurDir => return metadata(comps.as_path().to_str()?, pwd).map(|m| (m, false)),
        Component::ParentDir => {
            return metadata(comps.as_path().parent()?.to_str()?, pwd).map(|m| (m, false));
        }
        Component::Normal(name) => (comps.as_path(), name),
        _ => return None,
    };

    for entry in parent.read_dir().ok()? {
        let Ok(entry) = entry else {
            continue;
        };
        if entry
            .file_name()
            .as_encoded_bytes()
            .starts_with(name.as_encoded_bytes())
        {
            return entry.metadata().ok().map(|m| {
                (
                    m,
                    entry.file_name().as_encoded_bytes() != name.as_encoded_bytes(),
                )
            });
        }
    }

    None
}

/// Get the metadata of the given path
/// * If the path is relative, it is resolved against the provided `pwd`.
/// * If the path does not exist or the user lacks permission to access it, the
///   function returns `None`.
fn metadata(path: &str, pwd: &str) -> Option<Metadata> {
    let path = Path::new(path);
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        Path::new(pwd).join(path)
    };
    fs::metadata(&path).ok()
}

/// Get the type of the given path (file or directory).
///
/// * If the path is relative, it is resolved against the provided `pwd`.
/// * If `partial` is `true`, the function will attempt to find a file or
///   directory that starts with the given path.
/// * If the path does not exist or the user lacks permission to access it, the
///   function returns `None`.
///
/// Returns a tuple of the path type and a boolean indicating whether the path
/// was matched partially (i.e. if `partial` is `true` and the path was found by
/// prefix).
pub fn path_type(
    path: &str,
    pwd: &str,
    cdpath: Option<&[&str]>,
    partial: bool,
) -> Option<(PathType, bool)> {
    let (metadata, matched_partially) = std::iter::once(pwd)
        .chain(cdpath.into_iter().flatten().copied())
        .find_map(|pwd| {
            if partial && !path.ends_with('/') {
                find_by_prefix(path, pwd)
            } else {
                metadata(path, pwd).map(|m| (m, false))
            }
        })?;
    Some(if metadata.is_dir() {
        (PathType::Directory, matched_partially)
    } else {
        (PathType::File, matched_partially)
    })
}

/// Check if the given path is an executable file/directory.
/// * If the path is relative, it is resolved against the provided `pwd`.
/// * If `autocd` is `true` and the path is a traversable directory, it is
///   considered executable, mirroring Zsh's `AUTO_CD` option.
/// * Otherwise, directories are not executable commands.
pub fn is_path_executable(path: &str, pwd: &str, autocd: bool) -> bool {
    let Some(metadata) = metadata(path, pwd) else {
        return false;
    };
    let is_executable = (metadata.permissions().mode() & 0o111) != 0;
    is_executable && (!metadata.is_dir() || autocd)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs::{self, Permissions};
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn parse_nameddirs() {
        assert_eq!(
            named_directory("~some_dir-1.0/testfile"),
            Some("some_dir-1.0")
        );
        assert_eq!(named_directory("~some+dir/testfile"), None);
        assert_eq!(named_directory("~/testfile"), None);
        assert_eq!(named_directory("hel~lo"), None);
    }

    #[test]
    fn metadata_absolute_path() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("testfile");
        fs::write(&file_path, "hello").unwrap();

        let md = metadata(file_path.to_str().unwrap(), "/this/path/does/not/exist");
        assert!(md.is_some());
        assert!(md.unwrap().is_file());
    }

    #[test]
    fn metadata_relative_path() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("relative.txt");
        fs::write(&file_path, "data").unwrap();

        let md = metadata("relative.txt", dir.path().to_str().unwrap());
        assert!(md.is_some());
        assert!(md.unwrap().is_file());
    }

    #[test]
    fn metadata_nonexistent_path() {
        let md = metadata("/this/path/does/not/exist", "/");
        assert!(md.is_none());
    }

    #[test]
    fn path_type_file() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("afile");
        fs::write(&file_path, "content").unwrap();

        assert_eq!(
            path_type(file_path.to_str().unwrap(), "/", None, false),
            Some((PathType::File, false))
        );
    }

    #[test]
    fn path_type_directory() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("subdir");
        fs::create_dir(&sub).unwrap();

        assert_eq!(
            path_type(sub.to_str().unwrap(), "/", None, false),
            Some((PathType::Directory, false))
        );
    }

    #[test]
    fn path_type_cdpath() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("subdir")).unwrap();

        assert_eq!(
            path_type("subdir", "/", Some(&[dir.path().to_str().unwrap()]), false,),
            Some((PathType::Directory, false))
        );
    }

    #[test]
    fn path_type_file_partial() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("afile");
        fs::write(&file_path, "content").unwrap();

        assert_eq!(
            path_type("afi", dir.path().to_str().unwrap(), None, false),
            None
        );
        assert_eq!(
            path_type("afile/", dir.path().to_str().unwrap(), None, false),
            None
        );

        assert_eq!(
            path_type("afi", dir.path().to_str().unwrap(), None, true),
            Some((PathType::File, true))
        );
        assert_eq!(
            path_type("afile", dir.path().to_str().unwrap(), None, true),
            Some((PathType::File, false))
        );

        let sub = dir.path().join("subdir");
        fs::create_dir(&sub).unwrap();

        assert_eq!(
            path_type("../afi", sub.to_str().unwrap(), None, true),
            Some((PathType::File, true))
        );
    }

    #[test]
    fn path_type_directory_partial() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("subdir");
        fs::create_dir(&sub).unwrap();

        assert_eq!(
            path_type("sub", dir.path().to_str().unwrap(), None, false),
            None
        );

        assert_eq!(
            path_type("sub", dir.path().to_str().unwrap(), None, true),
            Some((PathType::Directory, true))
        );

        assert_eq!(
            path_type("subdir", dir.path().to_str().unwrap(), None, true),
            Some((PathType::Directory, false))
        );
        assert_eq!(
            path_type("subdir/", dir.path().to_str().unwrap(), None, true),
            Some((PathType::Directory, false))
        );

        assert_eq!(
            path_type(".", sub.to_str().unwrap(), None, true),
            Some((PathType::Directory, false))
        );
        assert_eq!(
            path_type("..", sub.to_str().unwrap(), None, true),
            Some((PathType::Directory, false))
        );
    }

    #[test]
    fn is_path_executable_executable_file() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("script.sh");
        fs::write(&file_path, "#!/bin/sh").unwrap();
        fs::set_permissions(&file_path, Permissions::from_mode(0o755)).unwrap();

        assert!(is_path_executable(file_path.to_str().unwrap(), "/", false));
    }

    #[test]
    fn is_path_executable_non_executable_file() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("readme.txt");
        fs::write(&file_path, "hello").unwrap();
        fs::set_permissions(&file_path, Permissions::from_mode(0o644)).unwrap();

        assert!(!is_path_executable(file_path.to_str().unwrap(), "/", false));
    }

    #[test]
    fn is_path_executable_non_executable_dir() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("noexecdir");
        fs::create_dir(&sub).unwrap();
        fs::set_permissions(&sub, Permissions::from_mode(0o644)).unwrap();

        assert!(!is_path_executable(sub.to_str().unwrap(), "/", false));

        let pwd = dir.path().to_str().unwrap();
        assert!(!is_path_executable("noexecdir/", pwd, false));
        assert!(!is_path_executable("./noexecdir", pwd, false));
        assert!(!is_path_executable("./noexecdir/", pwd, false));
        assert!(!is_path_executable("noexecdir", pwd, false));
    }

    #[test]
    fn is_path_executable_dir_without_autocd() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("mydir");
        fs::create_dir(&sub).unwrap();

        let path = sub.to_str().unwrap();
        assert!(path.starts_with('/'));
        assert!(!is_path_executable(path, "/", false));

        assert!(!is_path_executable("../mydir", path, false));
        assert!(!is_path_executable("../mydir2", path, false));

        let pwd = dir.path().to_str().unwrap();
        assert!(!is_path_executable("mydir", pwd, false));
        assert!(!is_path_executable("mydir/", pwd, false));
        assert!(!is_path_executable("mydir2/", pwd, false));
        assert!(!is_path_executable("./mydir", pwd, false));
        assert!(!is_path_executable("./mydir2", pwd, false));
        assert!(!is_path_executable("./mydir/", pwd, false));
        assert!(!is_path_executable("./mydir2/", pwd, false));
    }

    #[test]
    fn is_path_executable_nonexistent() {
        assert!(!is_path_executable("/no/such/path", "/", false));
    }

    /// When `autocd` is enabled, a bare directory name (no prefix) is
    /// considered executable. Files and nonexistent paths are unaffected.
    #[test]
    fn is_path_executable_autocd() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("mydir");
        fs::create_dir(&sub).unwrap();
        let file_path = dir.path().join("script.sh");
        fs::write(&file_path, "#!/bin/sh").unwrap();
        fs::set_permissions(&file_path, Permissions::from_mode(0o755)).unwrap();

        let pwd = dir.path().to_str().unwrap();

        assert!(is_path_executable("mydir", pwd, true));
        assert!(!is_path_executable("mydir", pwd, false));

        assert!(is_path_executable("mydir/", pwd, true));
        assert!(is_path_executable("./mydir", pwd, true));

        assert!(!is_path_executable("nodir", pwd, true));

        assert!(is_path_executable("script.sh", pwd, true));

        let file_path2 = dir.path().join("readme.txt");
        fs::write(&file_path2, "hello").unwrap();
        fs::set_permissions(&file_path2, Permissions::from_mode(0o644)).unwrap();

        assert!(!is_path_executable("readme.txt", pwd, true));
    }
}
