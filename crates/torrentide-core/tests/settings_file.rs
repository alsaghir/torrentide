//! The settings file on disk, under a temporary portable root.

use std::{fs, num::NonZeroU16, path::PathBuf};

use torrentide_core::{LoadedSettings, Paths, Settings, SettingsFile};

fn defaults() -> Settings {
    Settings::with_defaults(
        PathBuf::from("/downloads"),
        NonZeroU16::new(51413).expect("non-zero"),
    )
}

fn settings_file() -> (tempfile::TempDir, SettingsFile) {
    let root = tempfile::tempdir().expect("temp dir");
    let file = SettingsFile::new(&Paths::portable(root.path()));
    (root, file)
}

#[test]
fn first_load_creates_the_file_with_the_defaults() {
    let (_root, file) = settings_file();

    let loaded = file.load_or_init(defaults).unwrap();

    assert!(matches!(loaded, LoadedSettings::Created(ref s) if *s == defaults()));
    assert!(file.path().is_file());
}

#[test]
fn saved_settings_load_unchanged() {
    let (_root, file) = settings_file();
    let mut settings = defaults();
    settings.bittorrent.dht = false;

    file.save(&settings).unwrap();
    let loaded = file.load_or_init(defaults).unwrap();

    assert!(matches!(loaded, LoadedSettings::Existing(ref s) if *s == settings));
}

#[test]
fn broken_file_is_moved_aside_and_replaced_by_the_defaults() {
    let (_root, file) = settings_file();
    file.save(&defaults()).unwrap();
    fs::write(file.path(), b"version = 1\n\xff").unwrap();

    let loaded = file.load_or_init(defaults).unwrap();

    let LoadedSettings::Recovered {
        settings, invalid, ..
    } = loaded
    else {
        panic!("expected recovery, got {loaded:?}");
    };
    assert_eq!(settings, defaults());
    assert_eq!(fs::read(&invalid).unwrap(), b"version = 1\n\xff");
    assert!(matches!(
        file.load_or_init(defaults).unwrap(),
        LoadedSettings::Existing(_)
    ));
}

#[test]
fn first_run_defaults_use_a_dynamic_port() {
    let root = tempfile::tempdir().expect("temp dir");

    let settings = Settings::first_run(&Paths::portable(root.path()));

    assert!(settings.connection.listen_port.get() >= 49152);
    assert!(settings.downloads.dir.is_absolute());
}
