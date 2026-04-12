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

/// Find a file or directory that starts with the given prefix and return its
/// metadata.
/// * If the prefix is relative, it is resolved against the provided `pwd`.
/// * If multiple files or directories match the prefix, the function returns
///   the first one returned by `read_dir`, which is not guaranteed to be in any
///   particular order.
/// * If the prefix does not match any file or directory, or if the user lacks
///   permission to access it, the function returns `None`.
fn find_by_prefix(prefix: &str, pwd: &str) -> Option<Metadata> {
    let path = Path::new(prefix);
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        Path::new(pwd).join(path)
    };

    let mut comps = path.components();
    let last = comps.next_back()?;
    let (parent, name) = match last {
        Component::CurDir => return metadata(comps.as_path().to_str()?, pwd),
        Component::ParentDir => return metadata(comps.as_path().parent()?.to_str()?, pwd),
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
            return entry.metadata().ok();
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
/// * If the path is relative, it is resolved against the provided `pwd`.
/// * If `partial` is `true`, the function will attempt to find a file or
///   directory that starts with the given path.
/// * If the path does not exist or the user lacks permission to access it, the
///   function returns `None`.
pub fn path_type(path: &str, pwd: &str, partial: bool) -> Option<PathType> {
    let metadata = if partial && !path.ends_with('/') {
        find_by_prefix(path, pwd)?
    } else {
        metadata(path, pwd)?
    };
    Some(if metadata.is_dir() {
        PathType::Directory
    } else {
        PathType::File
    })
}

/// Check if the given path is an executable file.
/// * If the path is relative, it is resolved against the provided `pwd`.
/// * If the path is a directory, it is only considered executable if it
///   contains a slash.
pub fn is_path_executable(path: &str, pwd: &str) -> bool {
    let Some(metadata) = metadata(path, pwd) else {
        return false;
    };
    let is_executable = (metadata.permissions().mode() & 0o111) != 0;
    if metadata.is_dir() {
        is_executable && path.contains('/')
    } else {
        is_executable
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs::{self, Permissions};
    use std::os::unix::fs::PermissionsExt;

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
            path_type(file_path.to_str().unwrap(), "/", false),
            Some(PathType::File)
        );
    }

    #[test]
    fn path_type_directory() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("subdir");
        fs::create_dir(&sub).unwrap();

        assert_eq!(
            path_type(sub.to_str().unwrap(), "/", false),
            Some(PathType::Directory)
        );
    }

    #[test]
    fn path_type_file_partial() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("afile");
        fs::write(&file_path, "content").unwrap();

        assert_eq!(path_type("afi", dir.path().to_str().unwrap(), false), None);
        assert_eq!(
            path_type("afile/", dir.path().to_str().unwrap(), false),
            None
        );

        assert_eq!(
            path_type("afi", dir.path().to_str().unwrap(), true),
            Some(PathType::File)
        );
        assert_eq!(
            path_type("afile", dir.path().to_str().unwrap(), true),
            Some(PathType::File)
        );

        let sub = dir.path().join("subdir");
        fs::create_dir(&sub).unwrap();

        assert_eq!(
            path_type("../afi", sub.to_str().unwrap(), true),
            Some(PathType::File)
        );
    }

    #[test]
    fn path_type_directory_partial() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("subdir");
        fs::create_dir(&sub).unwrap();

        assert_eq!(path_type("sub", dir.path().to_str().unwrap(), false), None);

        assert_eq!(
            path_type("sub", dir.path().to_str().unwrap(), true),
            Some(PathType::Directory)
        );

        assert_eq!(
            path_type("subdir", dir.path().to_str().unwrap(), true),
            Some(PathType::Directory)
        );
        assert_eq!(
            path_type("subdir/", dir.path().to_str().unwrap(), true),
            Some(PathType::Directory)
        );

        assert_eq!(
            path_type(".", sub.to_str().unwrap(), true),
            Some(PathType::Directory)
        );
        assert_eq!(
            path_type("..", sub.to_str().unwrap(), true),
            Some(PathType::Directory)
        );
    }

    #[test]
    fn is_path_executable_executable_file() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("script.sh");
        fs::write(&file_path, "#!/bin/sh").unwrap();
        fs::set_permissions(&file_path, Permissions::from_mode(0o755)).unwrap();

        assert!(is_path_executable(file_path.to_str().unwrap(), "/"));
    }

    #[test]
    fn is_path_executable_non_executable_file() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("readme.txt");
        fs::write(&file_path, "hello").unwrap();
        fs::set_permissions(&file_path, Permissions::from_mode(0o644)).unwrap();

        assert!(!is_path_executable(file_path.to_str().unwrap(), "/"));
    }

    #[test]
    fn is_path_executable_dir_with_slash() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("mydir");
        fs::create_dir(&sub).unwrap();

        let path = sub.to_str().unwrap();
        assert!(path.contains('/'));
        assert!(is_path_executable(path, "/"));
    }

    #[test]
    fn is_path_executable_nonexistent() {
        assert!(!is_path_executable("/no/such/path", "/"));
    }
}
