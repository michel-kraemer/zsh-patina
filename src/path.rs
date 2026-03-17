use std::{
    fs::{self, Metadata},
    os::unix::fs::PermissionsExt,
    path::Path,
};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PathType {
    File,
    Directory,
}

/// Get the metadata of the given path
/// * If the path is relative, it is resolved against the provided `pwd`.
/// * If the path starts with a tilde (~), it is resolved against the user's
///   home directory
/// * If the path does not exist or the user lacks permission to access it, the
///   function returns `None`.
fn metadata(path: &str, pwd: &str) -> Option<Metadata> {
    let path = shellexpand::tilde(path);
    let path = Path::new(path.as_ref());
    let path = if path.is_absolute() {
        path.to_path_buf()
    } else {
        Path::new(pwd).join(path)
    };
    fs::metadata(&path).ok()
}

/// Get the type of the given path (file or directory).
/// * If the path is relative, it is resolved against the provided `pwd`.
/// * If the path starts with a tilde (~), it is resolved against the user's
///   home directory
/// * If the path does not exist or the user lacks permission to access it, the
///   function returns `None`.
pub fn path_type(path: &str, pwd: &str) -> Option<PathType> {
    let metadata = metadata(path, pwd)?;
    Some(if metadata.is_dir() {
        PathType::Directory
    } else {
        PathType::File
    })
}

/// Check if the given path is an executable file.
/// * If the path is relative, it is resolved against the provided `pwd`.
/// * If the path starts with a tilde (~), it is resolved against the user's
///   home directory
/// * If the path is a directory, it is only considered executable if it ends
///   with a slash.
pub fn is_path_executable(path: &str, pwd: &str) -> bool {
    let Some(metadata) = metadata(path, pwd) else {
        return false;
    };
    let is_executable = (metadata.permissions().mode() & 0o111) != 0;
    if metadata.is_dir() {
        is_executable && path.ends_with('/')
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
    fn metadata_tilde_expansion() {
        let md = metadata("~/", "/this/path/does/not/exist");
        assert!(md.is_some());
        assert!(md.unwrap().is_dir());
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
            path_type(file_path.to_str().unwrap(), "/"),
            Some(PathType::File)
        );
    }

    #[test]
    fn path_type_directory() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("subdir");
        fs::create_dir(&sub).unwrap();

        assert_eq!(
            path_type(sub.to_str().unwrap(), "/"),
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

        let path_with_slash = format!("{}/", sub.to_str().unwrap());
        assert!(is_path_executable(&path_with_slash, "/"));
    }

    #[test]
    fn is_path_executable_dir_without_slash() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("mydir");
        fs::create_dir(&sub).unwrap();

        assert!(!is_path_executable(sub.to_str().unwrap(), "/"));
    }

    #[test]
    fn is_path_executable_nonexistent() {
        assert!(!is_path_executable("/no/such/path", "/"));
    }
}
