//! Column definitions, cell values, and the first-run torrent table layout.

use std::{cmp::Ordering, fmt};

use torrentide_core::{Torrent, TorrentState, TorrentStatus};

/// A stable key for identifying a column in saved layouts.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ColumnId(&'static str);

impl ColumnId {
    /// The torrent name column.
    pub const NAME: Self = Self::new("name");
    /// The torrent size column.
    pub const SIZE: Self = Self::new("size");
    /// The download progress column.
    pub const PROGRESS: Self = Self::new("progress");
    /// The download rate column.
    pub const DOWNLOAD_RATE: Self = Self::new("download_rate");
    /// The upload rate column.
    pub const UPLOAD_RATE: Self = Self::new("upload_rate");
    /// The lifetime upload ratio column.
    pub const RATIO: Self = Self::new("ratio");
    /// The torrent activity column.
    pub const STATE: Self = Self::new("state");

    /// Creates a key from a stable string.
    #[must_use]
    pub const fn new(key: &'static str) -> Self {
        Self(key)
    }

    /// Returns the key used in saved layouts.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

/// The edge against which a cell's contents are aligned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    /// The leading edge.
    Start,
    /// The trailing edge.
    End,
}

/// How a cell's contents are drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CellKind {
    /// A text label.
    Text,
    /// A progress bar.
    Progress,
}

/// Describes a torrent table column and how it gets its values.
#[derive(Clone, Copy, Debug)]
pub struct Column {
    /// The column's persistence key.
    pub id: ColumnId,
    /// The alignment of the cell contents.
    pub align: Align,
    /// The cell presentation.
    pub kind: CellKind,
    /// The initial width in logical pixels.
    pub default_width: u32,
    /// The minimum width in logical pixels.
    pub min_width: u32,
    /// Whether the column is initially shown.
    pub default_visible: bool,
    /// Extracts the sortable cell value from a torrent.
    pub value: fn(&Torrent) -> CellValue,
}

/// The displayed torrent activity, in ascending sort order.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Activity {
    /// Verifying local data.
    Checking,
    /// Downloading an incomplete torrent or one with unknown size.
    Downloading,
    /// Sharing a complete torrent.
    Seeding,
    /// Stopped by a pause.
    Paused,
    /// Stopped by an error.
    Failed,
}

impl Activity {
    /// Returns the activity's display label.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Checking => "Checking",
            Self::Downloading => "Downloading",
            Self::Seeding => "Seeding",
            Self::Paused => "Paused",
            Self::Failed => "Failed",
        }
    }
}

impl From<&TorrentStatus> for Activity {
    fn from(status: &TorrentStatus) -> Self {
        match status.state {
            TorrentState::Checking => Self::Checking,
            TorrentState::Paused => Self::Paused,
            TorrentState::Failed(_) => Self::Failed,
            TorrentState::Running if status.size > 0 && status.done >= status.size => Self::Seeding,
            TorrentState::Running => Self::Downloading,
        }
    }
}

/// A cell's value before formatting.
#[derive(Clone, Debug)]
pub enum CellValue {
    /// Text shown as stored and sorted case-insensitively.
    Text(String),
    /// A byte count.
    Bytes(u64),
    /// A rate in bytes per second; zero is displayed as blank.
    Rate(u64),
    /// An upload ratio, absent when the size is unknown or zero.
    Ratio(Option<f64>),
    /// A completed fraction, clamped to 0–1 for display.
    Progress(f64),
    /// The torrent's current activity.
    State(Activity),
}

impl CellValue {
    /// Compares cell values, using declaration order for different variants.
    #[must_use]
    pub fn compare(&self, other: &Self) -> Ordering {
        match (self, other) {
            (Self::Text(a), Self::Text(b)) => a
                .chars()
                .flat_map(char::to_lowercase)
                .cmp(b.chars().flat_map(char::to_lowercase))
                .then_with(|| a.cmp(b)),
            (Self::Bytes(a), Self::Bytes(b)) | (Self::Rate(a), Self::Rate(b)) => a.cmp(b),
            (Self::Ratio(Some(a)), Self::Ratio(Some(b)))
            | (Self::Progress(a), Self::Progress(b)) => a.total_cmp(b),
            (Self::Ratio(a), Self::Ratio(b)) => a.is_some().cmp(&b.is_some()),
            (Self::State(a), Self::State(b)) => a.cmp(b),
            _ => self.rank().cmp(&other.rank()),
        }
    }

    const fn rank(&self) -> u8 {
        match self {
            Self::Text(_) => 0,
            Self::Bytes(_) => 1,
            Self::Rate(_) => 2,
            Self::Ratio(_) => 3,
            Self::Progress(_) => 4,
            Self::State(_) => 5,
        }
    }
}

impl fmt::Display for CellValue {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Text(text) => f.write_str(text),
            Self::Bytes(bytes) => write_bytes(f, *bytes),
            Self::Rate(0) => Ok(()),
            Self::Rate(bytes) => {
                write_bytes(f, *bytes)?;
                f.write_str("/s")
            }
            Self::Ratio(Some(ratio)) => write!(f, "{ratio:.2}"),
            Self::Ratio(None) => f.write_str("—"),
            Self::Progress(fraction) => {
                let fraction = fraction.clamp(0.0, 1.0);
                let mut permille = (fraction * 1000.0).trunc();
                if fraction < 1.0 {
                    // Only a complete torrent may read 100.0%, even when the product rounds up.
                    permille = permille.min(999.0);
                }
                write!(f, "{:.1}%", permille / 10.0)
            }
            Self::State(activity) => f.write_str(activity.label()),
        }
    }
}

fn write_bytes(f: &mut fmt::Formatter<'_>, bytes: u64) -> fmt::Result {
    const SCALES: [(u64, &str); 4] = [
        (1 << 40, "TiB"),
        (1 << 30, "GiB"),
        (1 << 20, "MiB"),
        (1 << 10, "KiB"),
    ];
    if let Some((scale, unit)) = SCALES.into_iter().find(|&(scale, _)| bytes >= scale) {
        write!(f, "{:.1} {unit}", quotient(bytes, scale))
    } else {
        write!(f, "{bytes} B")
    }
}

#[expect(
    clippy::cast_precision_loss,
    reason = "GUI fractions and formatted byte counts do not require exact 64-bit precision"
)]
fn quotient(numerator: u64, denominator: u64) -> f64 {
    numerator as f64 / denominator as f64
}

/// Returns the built-in torrent columns in their initial display order.
#[must_use]
pub fn builtin() -> Vec<Column> {
    vec![
        Column {
            id: ColumnId::NAME,
            align: Align::Start,
            kind: CellKind::Text,
            default_width: 300,
            min_width: 80,
            default_visible: true,
            value: |torrent| {
                let status = &torrent.status;
                CellValue::Text(status.name.clone().unwrap_or_else(|| status.id.to_string()))
            },
        },
        Column {
            id: ColumnId::SIZE,
            align: Align::End,
            kind: CellKind::Text,
            default_width: 90,
            min_width: 50,
            default_visible: true,
            value: |torrent| CellValue::Bytes(torrent.status.size),
        },
        Column {
            id: ColumnId::PROGRESS,
            align: Align::Start,
            kind: CellKind::Progress,
            default_width: 160,
            min_width: 60,
            default_visible: true,
            value: |torrent| {
                let TorrentStatus { size, done, .. } = torrent.status;
                CellValue::Progress(if size == 0 { 0.0 } else { quotient(done, size) })
            },
        },
        Column {
            id: ColumnId::DOWNLOAD_RATE,
            align: Align::End,
            kind: CellKind::Text,
            default_width: 90,
            min_width: 50,
            default_visible: true,
            value: |torrent| CellValue::Rate(torrent.status.download_rate),
        },
        Column {
            id: ColumnId::UPLOAD_RATE,
            align: Align::End,
            kind: CellKind::Text,
            default_width: 90,
            min_width: 50,
            default_visible: true,
            value: |torrent| CellValue::Rate(torrent.status.upload_rate),
        },
        Column {
            id: ColumnId::RATIO,
            align: Align::End,
            kind: CellKind::Text,
            default_width: 70,
            min_width: 40,
            default_visible: true,
            value: |torrent| {
                let size = torrent.status.size;
                CellValue::Ratio((size != 0).then(|| quotient(torrent.uploaded, size)))
            },
        },
        Column {
            id: ColumnId::STATE,
            align: Align::Start,
            kind: CellKind::Text,
            default_width: 100,
            min_width: 60,
            default_visible: true,
            value: |torrent| CellValue::State(Activity::from(&torrent.status)),
        },
    ]
}

/// The direction in which a column's values are sorted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    /// Smallest values first.
    Ascending,
    /// Largest values first.
    Descending,
}

/// The active sort column and direction.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sort {
    /// The column whose values determine row order.
    pub column: ColumnId,
    /// The order in which the values are compared.
    pub direction: Direction,
}

/// The width and visibility of a column in the table.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ColumnLayout {
    /// The column this entry applies to.
    pub id: ColumnId,
    /// The width in logical pixels.
    pub width: u32,
    /// Whether the column is shown.
    pub visible: bool,
}

/// The table's column order, widths, visibility, and sort.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TableLayout {
    /// Layout entries in display order, including hidden columns.
    pub columns: Vec<ColumnLayout>,
    /// The active row sort.
    pub sort: Sort,
}

impl TableLayout {
    /// Builds the first-run layout from registry defaults, sorted by Name ascending.
    #[must_use]
    pub fn new(columns: &[Column]) -> Self {
        Self {
            columns: columns
                .iter()
                .map(|column| ColumnLayout {
                    id: column.id,
                    width: column.default_width,
                    visible: column.default_visible,
                })
                .collect(),
            sort: Sort {
                column: ColumnId::NAME,
                direction: Direction::Ascending,
            },
        }
    }
}
