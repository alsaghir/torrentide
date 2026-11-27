//! The `torrentide-cli` binary, run against a temporary portable root; peers only on loopback.

use std::fs;
use std::io::{self, BufRead, BufReader, Lines};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, Output, Stdio};

use tempfile::TempDir;
use torrentide_core::{Database, Paths, Settings, SettingsFile, TorrentId, logging};

fn cli(root: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_torrentide-cli"))
        .env_remove("RUST_LOG")
        .arg("--portable")
        .arg(root)
        .args(args)
        .output()
        .expect("run torrentide-cli")
}

fn stdout(output: Output) -> String {
    assert!(output.status.success(), "{output:?}");
    String::from_utf8(output.stdout).expect("UTF-8 output")
}

fn logs(root: &Path) -> Vec<String> {
    let dir = logging::dir(&Paths::portable(root));
    fs::read_dir(dir)
        .expect("log directory")
        .map(|entry| entry.expect("log directory entry").path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("torrentide"))
                && path.extension().is_some_and(|ext| ext == "log")
        })
        .map(|path| fs::read_to_string(path).expect("log file"))
        .collect()
}

/// A portable root whose settings keep the engine off the network.
fn root() -> TempDir {
    root_with(|_| {})
}

/// [`root`], with `change` applied to its settings before they are saved.
fn root_with(change: impl FnOnce(&mut Settings)) -> TempDir {
    let root = tempfile::tempdir().expect("temp dir");
    let paths = Paths::portable(root.path());
    let mut settings = Settings::first_run(&paths);
    settings.downloads.dir = root.path().join("downloads");
    settings.bittorrent.dht = false;
    settings.bittorrent.lsd = false;
    change(&mut settings);
    fs::create_dir_all(&settings.downloads.dir).expect("downloads dir");
    SettingsFile::new(&paths)
        .save(&settings)
        .expect("settings saved");
    root
}

/// A one-byte, one-file torrent without trackers.
fn torrent_file(root: &Path) -> PathBuf {
    let mut torrent = b"d4:infod6:lengthi1e4:name1:a12:piece lengthi16384e6:pieces20:".to_vec();
    torrent.extend([0; 20]);
    torrent.extend(b"ee");
    let path = root.join("a.torrent");
    fs::write(&path, torrent).expect("torrent written");
    path
}

fn add(root: &Path, file: &Path) -> Output {
    cli(
        root,
        &["add", "--paused", file.to_str().expect("UTF-8 path")],
    )
}

/// Adds `file` to start running with the next `run`; returns its id.
fn add_running(root: &Path, file: &Path) -> String {
    let output = cli(root, &["add", file.to_str().expect("UTF-8 path")]);
    stdout(output).trim_end().to_owned()
}

#[test]
fn list_on_a_fresh_root_prints_nothing() {
    let root = tempfile::tempdir().expect("temp dir");

    assert_eq!(stdout(cli(root.path(), &["list"])), "");
}

#[test]
fn a_failing_command_logs_its_error() {
    let root = tempfile::tempdir().expect("temp dir");
    let missing = root.path().join("missing.torrent");

    let output = add(root.path(), &missing);

    assert!(!output.status.success(), "{output:?}");
    let error = format!("cannot read {}:", missing.display());
    let stderr = String::from_utf8(output.stderr).expect("UTF-8 output");
    assert_eq!(stderr.matches(&error).count(), 1, "{stderr}");
    let logs = logs(root.path());
    let occurrences: usize = logs.iter().map(|log| log.matches(&error).count()).sum();
    assert_eq!(occurrences, 1, "{logs:?}");
}

#[test]
fn list_prints_rows_in_queue_order() {
    let root = tempfile::tempdir().expect("temp dir");
    let database = Database::open(&Paths::portable(root.path())).expect("open database");
    assert!(
        database
            .insert_torrent(TorrentId::from_info_hash([0xbb; 20]), 200)
            .unwrap()
    );
    assert!(
        database
            .insert_torrent(TorrentId::from_info_hash([0xaa; 20]), 100)
            .unwrap()
    );
    drop(database);

    let expected = format!("0\t{}\t200\n1\t{}\t100\n", "bb".repeat(20), "aa".repeat(20));
    assert_eq!(stdout(cli(root.path(), &["list"])), expected);
}

#[test]
fn add_prints_the_id_and_list_shows_it() {
    let root = root();
    let file = torrent_file(root.path());

    let id = stdout(add(root.path(), &file));
    let listed = stdout(cli(root.path(), &["list"]));

    let id = id.trim_end();
    assert_eq!(id.len(), 40, "{id}");
    assert!(listed.starts_with(&format!("0\t{id}\t")), "{listed}");
    assert_eq!(listed.lines().count(), 1, "{listed}");
}

#[test]
fn adding_an_invalid_file_fails_and_adds_nothing() {
    let root = root();
    let file = root.path().join("broken.torrent");
    fs::write(&file, "not a torrent").expect("file written");

    let output = add(root.path(), &file);

    assert!(!output.status.success(), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("UTF-8 output");
    assert!(stderr.contains("the engine failed"), "{stderr}");
    assert_eq!(stdout(cli(root.path(), &["list"])), "");
}

#[test]
fn adding_a_torrent_again_fails() {
    let root = root();
    let file = torrent_file(root.path());
    let id = stdout(add(root.path(), &file));

    let output = add(root.path(), &file);

    assert!(!output.status.success(), "{output:?}");
    let stderr = String::from_utf8(output.stderr).expect("UTF-8 output");
    assert!(stderr.contains("already added"), "{stderr}");
    assert_eq!(stdout(cli(root.path(), &["list"])).lines().count(), 1);
    assert!(stderr.contains(id.trim_end()), "{stderr}");
}

#[test]
fn run_prints_the_restored_torrents() {
    let root = root();
    let file = torrent_file(root.path());
    let id = stdout(add(root.path(), &file));
    let expected = format!("{}\tpaused", id.trim_end());
    let (mut run, mut lines) = spawn_run(root.path());

    wait_for_snapshot(&mut lines, &expected);

    run.kill().expect("run killed");
    run.wait().expect("run reaped");
}

/// Spawns `run` with its standard output piped and read line by line.
fn spawn_run(root: &Path) -> (Child, Lines<BufReader<ChildStdout>>) {
    let mut run = Command::new(env!("CARGO_BIN_EXE_torrentide-cli"))
        .arg("--portable")
        .arg(root)
        .arg("run")
        .stdout(Stdio::piped())
        .spawn()
        .expect("run torrentide-cli");
    let lines = BufReader::new(run.stdout.take().expect("piped stdout")).lines();
    (run, lines)
}

/// Reads snapshots until one is exactly `expected`, a single torrent line.
fn wait_for_snapshot(lines: &mut Lines<BufReader<ChildStdout>>, expected: &str) {
    // The torrent may show as checking before it settles.
    let mut seen = Vec::new();
    loop {
        let snapshot: Vec<String> = lines
            .by_ref()
            .map(|line: io::Result<String>| line.expect("UTF-8 line"))
            .take_while(|line| !line.is_empty())
            .collect();
        if snapshot == [expected] {
            return;
        }
        seen.push(snapshot);
        assert!(seen.len() < 10, "no `{expected}` in {seen:?}");
    }
}

#[cfg(unix)]
mod unix {
    use std::fs;
    use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, TcpListener};
    use std::num::NonZeroU16;
    use std::process::{Child, ExitStatus};
    use std::thread;
    use std::time::{Duration, Instant};

    use nix::sys::signal::{Signal, kill};
    use nix::unistd::Pid;
    use torrentide_testkit::{PAYLOAD_NAME, payload_torrent, seed};

    use super::{add_running, root_with, spawn_run, wait_for_snapshot};

    /// A port nothing listens on right now, on the address the engine binds.
    fn free_port() -> NonZeroU16 {
        let listener = TcpListener::bind((Ipv6Addr::UNSPECIFIED, 0)).expect("bind a free port");
        let port = listener.local_addr().expect("bound address").port();
        NonZeroU16::new(port).expect("the OS never assigns port 0")
    }

    fn terminate(child: &Child) {
        let pid = Pid::from_raw(i32::try_from(child.id()).expect("PIDs fit in i32"));
        kill(pid, Signal::SIGTERM).expect("SIGTERM sent");
    }

    fn exit_status_within(child: &mut Child, timeout: Duration) -> ExitStatus {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = child.try_wait().expect("child status") {
                return status;
            }
            assert!(Instant::now() < deadline, "still running after {timeout:?}");
            thread::sleep(Duration::from_millis(100));
        }
    }

    #[test]
    fn run_downloads_stops_on_sigterm_and_resumes() {
        // Drives the seeder in the background while the test blocks on the CLI process.
        let runtime = tokio::runtime::Runtime::new().expect("tokio runtime");
        let port = free_port();
        let root = root_with(|settings| settings.connection.listen_port = port);
        let seed_dir = tempfile::tempdir().expect("temp dir");
        let (content, torrent) = runtime.block_on(payload_torrent(seed_dir.path()));
        let file = root.path().join("payload.torrent");
        fs::write(&file, torrent.as_bytes().expect("torrent serialises")).expect("torrent written");
        let id = add_running(root.path(), &file);
        let running = format!("{id}\trunning");

        let (mut run, mut lines) = spawn_run(root.path());
        wait_for_snapshot(&mut lines, &running);
        let peer = SocketAddr::from((Ipv4Addr::LOCALHOST, port.get()));
        let _seeder = runtime.block_on(seed(seed_dir.path(), &torrent, peer));
        let downloaded = root.path().join("downloads").join(PAYLOAD_NAME);
        let deadline = Instant::now() + Duration::from_secs(60);
        while fs::read(&downloaded).ok().as_ref() != Some(&content) {
            assert!(Instant::now() < deadline, "not downloaded within 60 s");
            thread::sleep(Duration::from_millis(100));
        }
        terminate(&run);
        assert!(exit_status_within(&mut run, Duration::from_secs(10)).success());

        // Stopping pauses torrents in the engine only, so they come back running.
        let (mut run, mut lines) = spawn_run(root.path());
        wait_for_snapshot(&mut lines, &running);
        terminate(&run);
        assert!(exit_status_within(&mut run, Duration::from_secs(10)).success());
    }
}
