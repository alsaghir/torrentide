//! Measures table refreshes on Linux with 5,000 synthetic torrents.
//!
//! Run with `cargo run --release -p torrentide-gui --example table_load`.

use std::fs;
use std::rc::Rc;
use std::time::{Duration, Instant};

use slint::{Timer, TimerMode};
use torrentide_core::{Torrent, TorrentId, TorrentState, TorrentStatus};
use torrentide_gui::{RunError, Window, columns};

const ROWS: usize = 5_000;
const TICK: Duration = Duration::from_secs(1);
const REPORT_EVERY: Duration = Duration::from_secs(10);
const MIB: u64 = 1 << 20;
const STATES: [TorrentState; 3] = [
    TorrentState::Running,
    TorrentState::Paused,
    TorrentState::Checking,
];

fn next(seed: &mut u64) -> u64 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 7;
    *seed ^= *seed << 17;
    *seed
}

fn state(seed: &mut u64) -> TorrentState {
    STATES[index(seed, STATES.len())].clone()
}

fn index(seed: &mut u64, len: usize) -> usize {
    let len = u64::try_from(len).expect("sample size fits in u64");
    usize::try_from(next(seed) % len).expect("sample index is below the original usize bound")
}

fn torrent(number: usize, seed: &mut u64) -> Torrent {
    let mut hash = [0; 20];
    hash[..size_of::<usize>()].copy_from_slice(&number.to_le_bytes());
    let size = (next(seed) % 40_000 + 1) * MIB;
    let done = if number.is_multiple_of(5) {
        size
    } else {
        next(seed) % size
    };
    Torrent {
        status: TorrentStatus {
            id: TorrentId::from_info_hash(hash),
            name: Some(format!("torrent {:04}", number + 1)),
            state: state(seed),
            size,
            done,
            download_rate: 0,
            upload_rate: 0,
            session_uploaded: 0,
        },
        uploaded: next(seed) % (2 * size),
    }
}

fn mutate(torrents: &mut [Torrent], indices: &mut [usize], seed: &mut u64) {
    // A partial shuffle samples without replacement, so exactly 10% of rows are touched.
    for i in 0..indices.len() / 10 {
        let j = i + index(seed, indices.len() - i);
        indices.swap(i, j);
        let status = &mut torrents[indices[i]].status;
        status.download_rate = if status.done < status.size {
            next(seed) % (8 * MIB)
        } else {
            0
        };
        status.upload_rate = next(seed) % MIB;
        status.done = (status.done + status.download_rate).min(status.size);
        if next(seed).is_multiple_of(16) {
            status.state = state(seed);
        }
    }
}

fn cpu_seconds() -> f64 {
    // Linux exposes these procfs counters in USER_HZ (100 ticks/s), not kernel HZ.
    const TICKS_PER_SECOND: f64 = 100.0;
    let stat = fs::read_to_string("/proc/self/stat").expect("the harness requires Linux procfs");
    // The command name may contain spaces and parentheses, so split after its closing delimiter.
    let (_, fields) = stat
        .rsplit_once(')')
        .expect("procfs encloses comm in parentheses");
    let mut times = fields.split_whitespace().skip(11);
    let user: f64 = times
        .next()
        .expect("procfs has utime")
        .parse()
        .expect("utime is numeric");
    let system: f64 = times
        .next()
        .expect("procfs has stime")
        .parse()
        .expect("stime is numeric");
    (user + system) / TICKS_PER_SECOND
}

fn main() -> Result<(), RunError> {
    let window = Window::new(columns::builtin())?;
    let mut seed = 0x9E37_79B9_7F4A_7C15;
    let mut torrents: Vec<_> = (0..ROWS).map(|id| torrent(id, &mut seed)).collect();
    let mut indices: Vec<_> = (0..ROWS).collect();
    window.show_torrents(&torrents);

    let timer = Timer::default();
    let display = Rc::clone(&window);
    let mut last_report = Instant::now();
    let mut last_cpu = cpu_seconds();
    let mut count = 0;
    let mut total = Duration::ZERO;
    timer.start(TimerMode::Repeated, TICK, move || {
        mutate(&mut torrents, &mut indices, &mut seed);
        let started = Instant::now();
        display.show_torrents(&torrents);
        total += started.elapsed();
        count += 1;

        let now = Instant::now();
        let elapsed = now.duration_since(last_report);
        if elapsed >= REPORT_EVERY {
            let cpu = cpu_seconds();
            let percent = (cpu - last_cpu) / elapsed.as_secs_f64() * 100.0;
            let average = total / count;
            println!("cpu {percent:5.1} %  applies {count:2}  avg apply {average:?}");
            last_report = now;
            last_cpu = cpu;
            count = 0;
            total = Duration::ZERO;
        }
    });

    window.run()
}
