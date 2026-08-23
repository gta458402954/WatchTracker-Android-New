use std::fs::{self, OpenOptions};
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use tauri::{AppHandle, Manager};

static NEXT_WRITE_PROBE_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DataMode {
    #[cfg(not(target_os = "android"))]
    Portable,
    AppData,
}

impl DataMode {
    pub fn as_str(self) -> &'static str {
        match self {
            #[cfg(not(target_os = "android"))]
            Self::Portable => "portable",
            Self::AppData => "app-data",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AppPaths {
    root: PathBuf,
    database: PathBuf,
    log: PathBuf,
    posters: PathBuf,
    backups: PathBuf,
    mode: DataMode,
}

impl AppPaths {
    pub fn resolve(app_handle: &AppHandle) -> Result<Self, String> {
        let app_data_dir = app_handle.path().app_data_dir().map_err(|error| {
            format!("Could not resolve the application data directory: {error}")
        })?;
        // Android's sandbox is the only supported data root on mobile.  In
        // particular, never inspect the native library directory for the
        // desktop portable `data/` convention.
        #[cfg(target_os = "android")]
        {
            return Self::resolve_from_android(&app_data_dir);
        }

        #[cfg(not(target_os = "android"))]
        {
            let executable = std::env::current_exe().ok();
            Self::resolve_from(executable.as_deref(), &app_data_dir)
        }
    }

    #[cfg(target_os = "android")]
    fn resolve_from_android(app_data_dir: &Path) -> Result<Self, String> {
        let root = app_data_dir.to_path_buf();
        if root.as_os_str().is_empty() {
            return Err("Resolved Android application data directory is empty".to_string());
        }
        ensure_directory(&root, "Android application data")?;
        let posters = root.join("posters");
        let backups = root.join("backups");
        ensure_directory(&posters, "poster")?;
        ensure_directory(&backups, "backup")?;
        verify_writable_directory(&root).map_err(|error| {
            format!(
                "The Android application data directory is not writable at {}: {error}",
                root.display()
            )
        })?;
        Ok(Self {
            database: root.join("watchtracker.db"),
            log: root.join("app.log"),
            posters,
            backups,
            root,
            mode: DataMode::AppData,
        })
    }

    #[cfg(not(target_os = "android"))]
    pub(crate) fn resolve_from(
        executable: Option<&Path>,
        app_data_dir: &Path,
    ) -> Result<Self, String> {
        Self::resolve_from_with_probe(executable, app_data_dir, verify_writable_directory)
    }

    #[cfg(not(target_os = "android"))]
    fn resolve_from_with_probe<F>(
        executable: Option<&Path>,
        app_data_dir: &Path,
        writable_probe: F,
    ) -> Result<Self, String>
    where
        F: FnOnce(&Path) -> io::Result<()>,
    {
        let portable_candidate = executable
            .and_then(Path::parent)
            .filter(|parent| !parent.as_os_str().is_empty())
            .map(|parent| parent.join("data"));

        let (root, mode) = match portable_candidate {
            Some(candidate) if candidate.is_dir() => (candidate, DataMode::Portable),
            Some(candidate) if candidate.exists() => {
                return Err(format!(
                    "Portable data path exists but is not a directory: {}",
                    candidate.display()
                ));
            }
            _ => (app_data_dir.to_path_buf(), DataMode::AppData),
        };

        if root.as_os_str().is_empty() {
            return Err("Resolved application data directory is empty".to_string());
        }

        ensure_directory(&root, "application data")?;
        let posters = root.join("posters");
        let backups = root.join("backups");
        ensure_directory(&posters, "poster")?;
        ensure_directory(&backups, "backup")?;
        writable_probe(&root).map_err(|error| {
            format!(
                "The selected {} data directory is not writable at {}: {error}",
                mode.as_str(),
                root.display()
            )
        })?;

        Ok(Self {
            database: root.join("watchtracker.db"),
            log: root.join("app.log"),
            posters,
            backups,
            root,
            mode,
        })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn database(&self) -> &Path {
        &self.database
    }

    pub fn log(&self) -> &Path {
        &self.log
    }

    pub fn posters(&self) -> &Path {
        &self.posters
    }

    pub fn backups(&self) -> &Path {
        &self.backups
    }

    pub fn mode(&self) -> DataMode {
        self.mode
    }

    pub fn poster_file(&self, file_name: &str) -> Result<PathBuf, String> {
        let mut components = Path::new(file_name).components();
        let is_safe =
            matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none();

        if !is_safe {
            return Err("Poster path must contain one safe file name".to_string());
        }

        Ok(self.posters.join(file_name))
    }
}

fn ensure_directory(path: &Path, label: &str) -> Result<(), String> {
    fs::create_dir_all(path).map_err(|error| {
        format!(
            "Could not prepare the {label} directory at {}: {error}",
            path.display()
        )
    })?;

    if !path.is_dir() {
        return Err(format!(
            "The {label} path is not a directory: {}",
            path.display()
        ));
    }

    Ok(())
}

fn verify_writable_directory(path: &Path) -> io::Result<()> {
    let probe_id = NEXT_WRITE_PROBE_ID.fetch_add(1, Ordering::Relaxed);
    let probe_path = path.join(format!(
        ".watchtracker-write-probe-{}-{probe_id}",
        std::process::id()
    ));
    let probe = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe_path)?;
    drop(probe);
    fs::remove_file(probe_path)
}

#[cfg(test)]
mod tests {
    use super::{AppPaths, DataMode};
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEST_ID: AtomicU64 = AtomicU64::new(1);

    struct TestRoot(PathBuf);

    impl TestRoot {
        fn new(name: &str) -> Self {
            let id = NEXT_TEST_ID.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir().join(format!(
                "watchtracker-app-paths-{}-{name}-{id}",
                std::process::id()
            ));
            fs::create_dir_all(&root).expect("create test root");
            Self(root)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TestRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn preexisting_data_directory_enables_portable_mode() {
        let test_root = TestRoot::new("portable");
        let executable_dir = test_root.path().join("bin");
        let portable = executable_dir.join("data");
        fs::create_dir_all(&portable).expect("create portable directory");

        let paths = AppPaths::resolve_from(
            Some(&executable_dir.join("app.exe")),
            &test_root.path().join("app-data"),
        )
        .expect("resolve portable paths");

        assert_eq!(paths.mode(), DataMode::Portable);
        assert_eq!(paths.root(), portable);
        assert_eq!(paths.database(), portable.join("watchtracker.db"));
        assert_eq!(paths.log(), portable.join("app.log"));
        assert_eq!(paths.posters(), portable.join("posters"));
        assert_eq!(paths.backups(), portable.join("backups"));
        assert!(paths.posters().is_dir());
        assert!(paths.backups().is_dir());
    }

    #[test]
    fn missing_portable_directory_uses_app_data_without_creating_portable_data() {
        let test_root = TestRoot::new("fallback");
        let executable_dir = test_root.path().join("bin");
        fs::create_dir_all(&executable_dir).expect("create executable directory");
        let app_data = test_root.path().join("app-data");

        let paths = AppPaths::resolve_from(Some(&executable_dir.join("app.exe")), &app_data)
            .expect("resolve app-data paths");

        assert_eq!(paths.mode(), DataMode::AppData);
        assert_eq!(paths.root(), app_data);
        assert!(!executable_dir.join("data").exists());
    }

    #[test]
    fn unavailable_executable_path_uses_app_data() {
        let test_root = TestRoot::new("no-executable");
        let app_data = test_root.path().join("app-data");

        let paths = AppPaths::resolve_from(None, &app_data).expect("resolve app-data paths");

        assert_eq!(paths.mode(), DataMode::AppData);
        assert_eq!(paths.root(), app_data);
    }

    #[test]
    fn reserved_portable_path_that_is_a_file_is_an_error() {
        let test_root = TestRoot::new("portable-file");
        let executable_dir = test_root.path().join("bin");
        fs::create_dir_all(&executable_dir).expect("create executable directory");
        fs::write(executable_dir.join("data"), b"not a directory")
            .expect("create conflicting file");

        let error = AppPaths::resolve_from(
            Some(&executable_dir.join("app.exe")),
            &test_root.path().join("app-data"),
        )
        .expect_err("conflicting portable path must fail");

        assert!(error.contains("not a directory"));
        assert!(!test_root.path().join("app-data").exists());
    }

    #[test]
    fn invalid_app_data_path_is_reported() {
        let test_root = TestRoot::new("invalid-app-data");
        let app_data = test_root.path().join("app-data");
        fs::write(&app_data, b"not a directory").expect("create conflicting file");

        let error =
            AppPaths::resolve_from(None, &app_data).expect_err("invalid app-data path must fail");

        assert!(error.contains("application data"));
        assert!(error.contains(&app_data.display().to_string()));
    }

    #[test]
    fn invalid_child_directory_is_reported() {
        let test_root = TestRoot::new("invalid-posters");
        let app_data = test_root.path().join("app-data");
        fs::create_dir_all(&app_data).expect("create app-data directory");
        fs::write(app_data.join("posters"), b"not a directory")
            .expect("create conflicting poster file");

        let error = AppPaths::resolve_from(None, &app_data)
            .expect_err("invalid poster directory must fail");

        assert!(error.contains("poster"));
    }

    #[test]
    fn unwritable_portable_directory_is_not_silently_redirected() {
        let test_root = TestRoot::new("unwritable-portable");
        let executable_dir = test_root.path().join("bin");
        let portable = executable_dir.join("data");
        fs::create_dir_all(&portable).expect("create portable directory");
        let app_data = test_root.path().join("app-data");

        let error = AppPaths::resolve_from_with_probe(
            Some(&executable_dir.join("app.exe")),
            &app_data,
            |_| {
                Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    "simulated read-only directory",
                ))
            },
        )
        .expect_err("unwritable portable directory must fail");

        assert!(error.contains("portable"));
        assert!(error.contains("not writable"));
        assert!(!app_data.exists());
    }

    #[test]
    fn poster_file_rejects_traversal_and_nested_paths() {
        let test_root = TestRoot::new("poster-safety");
        let paths = AppPaths::resolve_from(None, &test_root.path().join("app-data"))
            .expect("resolve paths");

        assert_eq!(
            paths.poster_file("cover.jpg").expect("safe poster"),
            paths.posters().join("cover.jpg")
        );
        let poster = paths.poster_file("cover.jpg").expect("safe poster");
        fs::write(&poster, b"synthetic poster bytes").expect("write poster fixture");
        assert_eq!(
            fs::read(poster).expect("read poster fixture"),
            b"synthetic poster bytes"
        );
        for unsafe_name in ["", "../secret.jpg", "nested/cover.jpg", "/absolute.jpg"] {
            assert!(paths.poster_file(unsafe_name).is_err(), "{unsafe_name}");
        }
    }
}
