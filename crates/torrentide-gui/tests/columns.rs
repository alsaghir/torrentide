use std::cmp::Ordering;

use torrentide_core::{Torrent, TorrentId, TorrentState, TorrentStatus};
use torrentide_gui::columns::{
    self, Activity, Align, CellKind, CellValue, ColumnId, Direction, Sort, TableLayout,
};

fn torrent() -> Torrent {
    Torrent {
        status: TorrentStatus {
            id: TorrentId::from_info_hash([0xab; 20]),
            name: Some("Example".into()),
            state: TorrentState::Running,
            size: 2048,
            done: 1024,
            download_rate: 4096,
            upload_rate: 256,
            session_uploaded: 1024,
        },
        uploaded: 2560,
    }
}

fn cell(id: ColumnId, torrent: &Torrent) -> CellValue {
    let column = columns::builtin()
        .into_iter()
        .find(|column| column.id == id)
        .expect("built-in column exists");
    (column.value)(torrent)
}

#[test]
fn builtin_columns_have_distinct_keys_and_match_header_order() {
    use Align::{End, Start};
    use CellKind::{Progress, Text};

    let metadata: Vec<_> = columns::builtin()
        .iter()
        .map(|column| {
            (
                column.id.as_str(),
                column.align,
                column.kind,
                column.min_width,
            )
        })
        .collect();
    assert_eq!(
        metadata,
        [
            ("name", Start, Text, 80),
            ("size", End, Text, 50),
            ("progress", Start, Progress, 60),
            ("download_rate", End, Text, 50),
            ("upload_rate", End, Text, 50),
            ("ratio", End, Text, 40),
            ("state", Start, Text, 60),
        ]
    );
}

#[test]
fn builtin_values_are_typed_and_ratio_uses_lifetime_uploads() {
    let torrent = torrent();
    let expected = [
        CellValue::Text("Example".into()),
        CellValue::Bytes(2048),
        CellValue::Progress(0.5),
        CellValue::Rate(4096),
        CellValue::Rate(256),
        CellValue::Ratio(Some(1.25)),
        CellValue::State(Activity::Downloading),
    ];
    let columns = columns::builtin();
    assert_eq!(columns.len(), expected.len());
    for (column, expected) in columns.iter().zip(expected) {
        assert_eq!(
            (column.value)(&torrent).compare(&expected),
            Ordering::Equal,
            "unexpected value in {}",
            column.id.as_str()
        );
    }
}

#[test]
fn missing_metadata_uses_hash_and_has_no_ratio_or_progress() {
    let mut torrent = torrent();
    torrent.status.name = None;
    torrent.status.size = 0;
    let expected = [
        (ColumnId::NAME, CellValue::Text("ab".repeat(20))),
        (ColumnId::RATIO, CellValue::Ratio(None)),
        (ColumnId::PROGRESS, CellValue::Progress(0.0)),
    ];
    for (id, value) in expected {
        assert_eq!(
            cell(id, &torrent).compare(&value),
            Ordering::Equal,
            "{id:?}"
        );
    }
}

#[test]
fn activity_reflects_engine_state_and_completion() {
    let cases = [
        (TorrentState::Checking, 2, 1, Activity::Checking, "Checking"),
        (TorrentState::Paused, 2, 2, Activity::Paused, "Paused"),
        (
            TorrentState::Failed("disk full".into()),
            2,
            1,
            Activity::Failed,
            "Failed",
        ),
        (
            TorrentState::Running,
            2,
            1,
            Activity::Downloading,
            "Downloading",
        ),
        (TorrentState::Running, 2, 2, Activity::Seeding, "Seeding"),
        (TorrentState::Running, 2, 3, Activity::Seeding, "Seeding"),
        (
            TorrentState::Running,
            0,
            0,
            Activity::Downloading,
            "Downloading",
        ),
    ];
    let mut torrent = torrent();
    for (state, size, done, activity, label) in cases {
        torrent.status.state = state;
        torrent.status.size = size;
        torrent.status.done = done;
        let value = cell(ColumnId::STATE, &torrent);
        assert_eq!(value.compare(&CellValue::State(activity)), Ordering::Equal);
        assert_eq!(value.to_string(), label);
    }
}

#[test]
fn formats_cell_values_for_display() {
    let cases = [
        (CellValue::Bytes(0), "0 B"),
        (CellValue::Bytes(999), "999 B"),
        (CellValue::Bytes(1023), "1023 B"),
        (CellValue::Bytes(1024), "1.0 KiB"),
        (CellValue::Bytes(1 << 20), "1.0 MiB"),
        (CellValue::Bytes(3 << 19), "1.5 MiB"),
        (CellValue::Bytes(2 << 30), "2.0 GiB"),
        (CellValue::Bytes(1 << 40), "1.0 TiB"),
        (CellValue::Bytes(1 << 50), "1024.0 TiB"),
        (CellValue::Rate(0), ""),
        (CellValue::Rate(3 << 19), "1.5 MiB/s"),
        (CellValue::Ratio(Some(0.0)), "0.00"),
        (CellValue::Ratio(Some(1.25)), "1.25"),
        (CellValue::Ratio(None), "—"),
        (CellValue::Progress(-0.2), "0.0%"),
        (CellValue::Progress(0.5), "50.0%"),
        (CellValue::Progress(0.99999), "99.9%"),
        (CellValue::Progress(1.0), "100.0%"),
        (CellValue::Progress(1.2), "100.0%"),
        (CellValue::Text("Some text".into()), "Some text"),
    ];
    for (value, label) in cases {
        assert_eq!(value.to_string(), label, "{value:?}");
    }
}

#[test]
fn sorts_text_case_insensitively_with_case_sensitive_ties() {
    let mut values: Vec<_> = ["gamma", "a", "Beta", "alpha", "A"]
        .map(|text| CellValue::Text(text.into()))
        .into();
    values.sort_by(CellValue::compare);
    assert_eq!(
        values.iter().map(ToString::to_string).collect::<Vec<_>>(),
        ["A", "a", "alpha", "Beta", "gamma"]
    );
    assert_eq!(values[0].compare(&values[1]), Ordering::Less);
}

#[test]
fn orders_numbers_activities_and_mixed_variants() {
    let ascending = [
        CellValue::Text("last".into()),
        CellValue::Bytes(999),
        CellValue::Bytes(1024),
        CellValue::Rate(999),
        CellValue::Rate(1024),
        CellValue::Ratio(None),
        CellValue::Ratio(Some(-0.0)),
        CellValue::Ratio(Some(0.0)),
        CellValue::Progress(0.9),
        CellValue::Progress(1.0),
        CellValue::State(Activity::Checking),
        CellValue::State(Activity::Downloading),
        CellValue::State(Activity::Seeding),
        CellValue::State(Activity::Paused),
        CellValue::State(Activity::Failed),
    ];
    for pair in ascending.windows(2) {
        assert_eq!(pair[0].compare(&pair[1]), Ordering::Less, "{pair:?}");
        assert_eq!(pair[1].compare(&pair[0]), Ordering::Greater, "{pair:?}");
    }
}

#[test]
fn first_run_shows_builtin_columns_at_their_default_widths() {
    let layout = TableLayout::new(&columns::builtin());
    let entries: Vec<_> = layout
        .columns
        .iter()
        .map(|column| (column.id, column.width, column.visible))
        .collect();
    assert_eq!(
        entries,
        [
            (ColumnId::NAME, 300, true),
            (ColumnId::SIZE, 90, true),
            (ColumnId::PROGRESS, 160, true),
            (ColumnId::DOWNLOAD_RATE, 90, true),
            (ColumnId::UPLOAD_RATE, 90, true),
            (ColumnId::RATIO, 70, true),
            (ColumnId::STATE, 100, true),
        ]
    );
    assert_eq!(
        layout.sort,
        Sort {
            column: ColumnId::NAME,
            direction: Direction::Ascending,
        }
    );
}
