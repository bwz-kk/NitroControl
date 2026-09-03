//! Testing seam for filesystem access, per docs/architecture.md's "Testing seams" section.
//!
//! Every provider reads hardware state through this trait instead of touching
//! `std::fs` directly, so unit tests can inject fixture strings for valid values,
//! malformed values, missing files, and permission-denied errors.

use std::io;
use std::path::{Path, PathBuf};

/// Abstracts reading sysfs/procfs-style files and listing directories.
pub trait SysfsReader: Send + Sync {
    fn read_to_string(&self, path: &Path) -> io::Result<String>;
    fn read_dir(&self, path: &Path) -> io::Result<Vec<PathBuf>>;
}

/// Reads the real filesystem. Used in production.
pub struct RealSysfsReader;

impl SysfsReader for RealSysfsReader {
    fn read_to_string(&self, path: &Path) -> io::Result<String> {
        std::fs::read_to_string(path)
    }

    fn read_dir(&self, path: &Path) -> io::Result<Vec<PathBuf>> {
        std::fs::read_dir(path)?
            .map(|entry| entry.map(|e| e.path()))
            .collect()
    }
}

/// In-memory [`SysfsReader`] for tests: lets test code inject fixture content,
/// simulate missing files, permission-denied errors, and sequential reads
/// (e.g. two successive `/proc/stat` snapshots for a rate calculation) without
/// touching the real filesystem.
#[cfg(test)]
pub mod mock {
    use super::*;
    use std::collections::HashMap;
    use std::collections::VecDeque;
    use std::sync::Mutex;

    enum Entry {
        Content(VecDeque<String>),
        PermissionDenied,
    }

    #[derive(Default)]
    pub struct MockSysfsReader {
        files: Mutex<HashMap<PathBuf, Entry>>,
        dirs: Mutex<HashMap<PathBuf, Vec<PathBuf>>>,
    }

    impl MockSysfsReader {
        pub fn new() -> Self {
            Self::default()
        }

        /// Every future read of `path` returns `content`.
        pub fn set_content(&self, path: impl Into<PathBuf>, content: impl Into<String>) {
            self.set_sequence(path, vec![content.into()]);
        }

        /// Successive reads of `path` return each string in `contents`, in order;
        /// once exhausted, the last value repeats.
        pub fn set_sequence(&self, path: impl Into<PathBuf>, contents: Vec<String>) {
            self.files
                .lock()
                .unwrap()
                .insert(path.into(), Entry::Content(contents.into()));
        }

        /// Future reads of `path` fail with `PermissionDenied`.
        pub fn set_permission_denied(&self, path: impl Into<PathBuf>) {
            self.files
                .lock()
                .unwrap()
                .insert(path.into(), Entry::PermissionDenied);
        }

        /// `read_dir(path)` returns `entries`.
        pub fn set_dir(&self, path: impl Into<PathBuf>, entries: Vec<PathBuf>) {
            self.dirs.lock().unwrap().insert(path.into(), entries);
        }
    }

    impl SysfsReader for MockSysfsReader {
        fn read_to_string(&self, path: &Path) -> io::Result<String> {
            let mut files = self.files.lock().unwrap();
            match files.get_mut(path) {
                Some(Entry::Content(queue)) => {
                    let next = if queue.len() > 1 {
                        queue.pop_front().unwrap()
                    } else {
                        queue.front().cloned().unwrap()
                    };
                    Ok(next)
                }
                Some(Entry::PermissionDenied) => {
                    Err(io::Error::from(io::ErrorKind::PermissionDenied))
                }
                None => Err(io::Error::from(io::ErrorKind::NotFound)),
            }
        }

        fn read_dir(&self, path: &Path) -> io::Result<Vec<PathBuf>> {
            self.dirs
                .lock()
                .unwrap()
                .get(path)
                .cloned()
                .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))
        }
    }

    #[cfg(test)]
    mod mock_tests {
        use super::*;

        #[test]
        fn missing_path_is_not_found() {
            let mock = MockSysfsReader::new();
            let err = mock.read_to_string(Path::new("/nope")).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::NotFound);
        }

        #[test]
        fn fixed_content_repeats_on_every_read() {
            let mock = MockSysfsReader::new();
            mock.set_content("/x", "55800\n");
            assert_eq!(mock.read_to_string(Path::new("/x")).unwrap(), "55800\n");
            assert_eq!(mock.read_to_string(Path::new("/x")).unwrap(), "55800\n");
        }

        #[test]
        fn sequence_advances_then_repeats_last_value() {
            let mock = MockSysfsReader::new();
            mock.set_sequence("/proc/stat", vec!["a".into(), "b".into()]);
            assert_eq!(mock.read_to_string(Path::new("/proc/stat")).unwrap(), "a");
            assert_eq!(mock.read_to_string(Path::new("/proc/stat")).unwrap(), "b");
            assert_eq!(mock.read_to_string(Path::new("/proc/stat")).unwrap(), "b");
        }

        #[test]
        fn permission_denied_path_errors() {
            let mock = MockSysfsReader::new();
            mock.set_permission_denied("/secret");
            let err = mock.read_to_string(Path::new("/secret")).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::PermissionDenied);
        }

        #[test]
        fn dir_listing_returns_configured_entries() {
            let mock = MockSysfsReader::new();
            mock.set_dir(
                "/sys/class/hwmon",
                vec![PathBuf::from("/sys/class/hwmon/hwmon0")],
            );
            assert_eq!(
                mock.read_dir(Path::new("/sys/class/hwmon")).unwrap(),
                vec![PathBuf::from("/sys/class/hwmon/hwmon0")]
            );
        }

        #[test]
        fn missing_dir_is_not_found() {
            let mock = MockSysfsReader::new();
            let err = mock.read_dir(Path::new("/nope")).unwrap_err();
            assert_eq!(err.kind(), io::ErrorKind::NotFound);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn real_reader_reads_an_actual_file() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("temp1_input");
        std::fs::write(&file_path, "55800\n").unwrap();

        let reader = RealSysfsReader;
        let contents = reader.read_to_string(&file_path).unwrap();

        assert_eq!(contents, "55800\n");
    }

    #[test]
    fn real_reader_lists_an_actual_directory() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("hwmon0"), "").unwrap();
        std::fs::write(dir.path().join("hwmon1"), "").unwrap();

        let reader = RealSysfsReader;
        let mut entries = reader.read_dir(dir.path()).unwrap();
        entries.sort();

        assert_eq!(
            entries,
            vec![dir.path().join("hwmon0"), dir.path().join("hwmon1")]
        );
    }

    #[test]
    fn real_reader_reports_missing_file_as_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let reader = RealSysfsReader;

        let err = reader
            .read_to_string(&dir.path().join("does_not_exist"))
            .unwrap_err();

        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }
}
