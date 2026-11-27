//! File logging under a portable root.

// These tests rely on nextest running each test in its own process so it can install a subscriber.

use std::{fs, thread};

use torrentide_core::logging::{self, LogError};
use torrentide_core::{APP_NAME, Paths, SettingsFile};

fn portable() -> (tempfile::TempDir, Paths) {
    let root = tempfile::tempdir().expect("temp dir");
    let paths = Paths::portable(root.path());
    (root, paths)
}

fn read_logs(paths: &Paths) -> String {
    let mut logs = String::new();
    for entry in fs::read_dir(logging::dir(paths)).unwrap() {
        logs.push_str(&fs::read_to_string(entry.unwrap().path()).unwrap());
    }
    logs
}

#[test]
fn info_events_from_torrentide_crates_reach_the_log_file() {
    let (_root, paths) = portable();
    let guard = logging::init(&paths, None).unwrap();
    tracing::info!(target: "torrentide_core::test", "core information");
    tracing::info!(target: "torrentide_engine_rqbit::test", "engine information");
    drop(guard);

    let logs = read_logs(&paths);
    assert!(logs.contains("core information"));
    assert!(logs.contains("engine information"));
    let entries = fs::read_dir(logging::dir(&paths)).unwrap();
    for entry in entries {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_str().unwrap();
        assert!(name.starts_with(APP_NAME), "unexpected log name: {name}");
        assert_eq!(path.extension().unwrap(), "log");
    }
}

#[test]
fn settings_loader_logs_creation_and_recovery() {
    let (_root, paths) = portable();
    let guard = logging::init(&paths, None).unwrap();
    let file = SettingsFile::new(&paths);
    SettingsFile::load_logged(&paths).unwrap();
    let invalid = "version = 2";
    fs::write(file.path(), invalid).unwrap();
    let recovered = SettingsFile::load_logged(&paths).unwrap();
    assert_eq!(SettingsFile::load_logged(&paths).unwrap(), recovered);
    drop(guard);

    let backup = fs::read_dir(paths.config_dir())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| path != file.path())
        .expect("an invalid settings backup");
    assert_eq!(fs::read_to_string(&backup).unwrap(), invalid);
    let logs = read_logs(&paths);
    assert!(logs.lines().any(|line| line.contains("INFO")
        && line.contains("wrote default settings")
        && line.contains(&format!("path={}", file.path().display()))));
    assert!(logs.lines().any(|line| line.contains("WARN")
        && line.contains("settings were invalid; using defaults")
        && line.contains(&format!("moved_to={}", backup.display()))
        && line.contains("error=settings version 2 is not supported")));
}

#[test]
fn other_crates_log_only_warnings_by_default() {
    let (_root, paths) = portable();
    let guard = logging::init(&paths, None).unwrap();
    tracing::info!(target: "librqbit::x", "dependency information");
    tracing::warn!(target: "librqbit::x", "dependency warning");
    drop(guard);

    let logs = read_logs(&paths);
    assert!(!logs.contains("dependency information"));
    assert!(logs.contains("dependency warning"));
}

#[test]
fn directives_replace_the_default_filter() {
    let (_root, paths) = portable();
    let guard = logging::init(&paths, Some("debug")).unwrap();
    tracing::debug!(target: "torrentide_core::t", "core debug event");
    drop(guard);

    assert!(read_logs(&paths).contains("core debug event"));
}

#[test]
fn invalid_directives_fall_back_to_the_default_with_a_warning() {
    let (_root, paths) = portable();
    let invalid = "torrentide=not-a-level";
    let guard = logging::init(&paths, Some(invalid)).unwrap();
    tracing::info!(target: "torrentide_core::test", "core information after fallback");
    tracing::info!(target: "librqbit::x", "dependency information after fallback");
    drop(guard);

    let logs = read_logs(&paths);
    assert!(logs.lines().any(|line| line.contains("WARN")
        && line.contains("invalid log directives")
        && line.contains(invalid)
        && line.contains("error=")));
    assert!(logs.contains("core information after fallback"));
    assert!(!logs.contains("dependency information after fallback"));
}

#[test]
fn panics_are_logged() {
    let (_root, paths) = portable();
    let guard = logging::init(&paths, None).unwrap();
    thread::spawn(|| panic!("logging test thread panicked"))
        .join()
        .expect_err("test thread should panic");
    drop(guard);

    assert!(read_logs(&paths).contains("logging test thread panicked"));
}

#[test]
fn a_second_init_fails() {
    let (_root, paths) = portable();
    let _guard = logging::init(&paths, None).unwrap();
    let error = logging::init(&paths, None).expect_err("logging should already be initialised");

    assert!(
        matches!(error, LogError::AlreadyInitialised(_)),
        "unexpected error: {error:?}"
    );
}
