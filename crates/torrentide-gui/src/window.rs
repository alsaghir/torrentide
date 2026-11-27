//! Adapts the column registry and torrent snapshots to the Slint table.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt::{self, Write as _};
use std::rc::Rc;

use slint::{ComponentHandle, EventLoopError, Model, ModelRc, PlatformError, VecModel, Weak};
use tokio::sync::watch;
use torrentide_core::Torrent;

use crate::columns::{Align, CellKind, CellValue, Column, Direction, Sort, TableLayout};
use crate::fluent::{self, Cell, ColumnSpec, MainWindow, TorrentRow};

/// The window system could not be started or failed while running.
#[derive(Debug, thiserror::Error)]
pub enum RunError {
    /// The window could not be created, shown, or run.
    #[error("the window system failed")]
    Platform(#[from] PlatformError),
    /// The snapshot task could not be scheduled on the window's event loop.
    #[error("cannot follow torrent snapshots")]
    Follow(#[from] EventLoopError),
}

/// The request to close the window could not be queued.
#[derive(Debug, thiserror::Error)]
#[error("cannot reach the window's event loop")]
pub struct CloseError(#[from] EventLoopError);

/// A handle that can request the window to close from any thread.
#[derive(Clone)]
pub struct Closer(Weak<MainWindow>);

impl fmt::Debug for Closer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Closer").finish_non_exhaustive()
    }
}

impl Closer {
    /// Requests that the window close, doing nothing if it has been dropped.
    ///
    /// # Errors
    ///
    /// Returns [`CloseError`] if the event loop cannot accept the request.
    pub fn close(&self) -> Result<(), CloseError> {
        self.0.upgrade_in_event_loop(|component| {
            // Slint releases the window's keepalive even on a hide error, and run retries the hide.
            let _ = component.hide();
        })?;
        Ok(())
    }
}

/// Displays torrent snapshots in a single-threaded Slint window.
pub struct Window {
    component: MainWindow,
    columns: Vec<Column>,
    sort: Sort,
    rows: Rc<VecModel<TorrentRow>>,
    shown: RefCell<Vec<ShownRow>>,
    snapshots: RefCell<Option<watch::Receiver<Vec<Torrent>>>>,
}

/// The torrent a row last displayed, so an unchanged torrent skips formatting entirely.
struct ShownRow {
    torrent: Torrent,
    cells: Rc<VecModel<Cell>>,
}

impl fmt::Debug for Window {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Window")
            .field("columns", &self.columns)
            .field("row_count", &self.rows.row_count())
            .finish_non_exhaustive()
    }
}

impl Window {
    /// Creates a window with the registry's default columns and no torrents.
    ///
    /// # Errors
    ///
    /// Returns [`RunError`] if the window system cannot create the window.
    pub fn new(columns: Vec<Column>) -> Result<Self, RunError> {
        let component = MainWindow::new()?;
        let layout = TableLayout::new(&columns);
        let (specs, columns) = visible_columns(columns, &layout);
        component.set_columns(ModelRc::from(Rc::new(VecModel::from(specs))));
        let rows = Rc::new(VecModel::default());
        component.set_rows(ModelRc::from(Rc::clone(&rows)));
        Ok(Self {
            component,
            columns,
            sort: layout.sort,
            rows,
            shown: RefCell::default(),
            snapshots: RefCell::default(),
        })
    }

    /// Returns a handle for closing this window from another thread.
    #[must_use]
    pub fn closer(&self) -> Closer {
        Closer(self.component.as_weak())
    }

    /// Returns the displayed cell texts in row and visible column order.
    #[cfg(any(test, feature = "testing"))]
    #[must_use]
    pub fn cell_texts(&self) -> Vec<Vec<String>> {
        self.component
            .get_rows()
            .iter()
            .map(|row| row.cells.iter().map(|cell| cell.text.to_string()).collect())
            .collect()
    }

    /// Displays torrents in layout sort order, updating only cells whose display changes.
    ///
    /// Keeps snapshot order for equal keys or if the sort column is not visible.
    pub fn show_torrents(&self, torrents: &[Torrent]) {
        let mut shown = self.shown.borrow_mut();
        while shown.len() > torrents.len() {
            self.rows.remove(shown.len() - 1);
            shown.pop();
        }

        let mut text = String::new();
        for (position, torrent) in self.sorted(torrents).into_iter().enumerate() {
            if let Some(row) = shown.get_mut(position) {
                if row.torrent == *torrent {
                    continue;
                }
                for (index, column) in self.columns.iter().enumerate() {
                    let fraction = format_cell(&(column.value)(torrent), &mut text);
                    if row.cells.row_data(index).is_some_and(|cell| {
                        cell.text.as_str() != text || cell.fraction.to_bits() != fraction.to_bits()
                    }) {
                        row.cells.set_row_data(index, make_cell(&text, fraction));
                    }
                }
                row.torrent.clone_from(torrent);
                continue;
            }

            let values: Vec<_> = self
                .columns
                .iter()
                .map(|column| {
                    let fraction = format_cell(&(column.value)(torrent), &mut text);
                    make_cell(&text, fraction)
                })
                .collect();
            let cells = Rc::new(VecModel::from(values));
            self.rows.push(TorrentRow {
                cells: ModelRc::from(Rc::clone(&cells)),
            });
            shown.push(ShownRow {
                torrent: torrent.clone(),
                cells,
            });
        }
    }

    fn sorted<'a>(&self, torrents: &'a [Torrent]) -> Vec<&'a Torrent> {
        let Some(column) = self
            .columns
            .iter()
            .find(|column| column.id == self.sort.column)
        else {
            return torrents.iter().collect();
        };
        // One key per torrent: extractors may allocate, so they stay out of the comparator.
        let mut keyed: Vec<_> = torrents
            .iter()
            .map(|torrent| ((column.value)(torrent), torrent))
            .collect();
        keyed.sort_by(|(left, _), (right, _)| {
            let ordering = left.compare(right);
            match self.sort.direction {
                Direction::Ascending => ordering,
                Direction::Descending => ordering.reverse(),
            }
        });
        keyed.into_iter().map(|(_, torrent)| torrent).collect()
    }

    /// Displays the current snapshot and follows updates on the window's event loop.
    ///
    /// Call on the window thread. Updates stop when the channel closes or the window is dropped.
    ///
    /// # Errors
    ///
    /// Returns [`RunError`] if the update task cannot be scheduled.
    pub fn follow(
        self: &Rc<Self>,
        mut snapshots: watch::Receiver<Vec<Torrent>>,
    ) -> Result<(), RunError> {
        // Release the channel's read lock before touching Slint so publishers aren't blocked.
        let snapshot = snapshots.borrow_and_update().clone();
        self.show_torrents(&snapshot);
        let latest = snapshots.clone();
        let window = Rc::downgrade(self);
        slint::spawn_local(async move {
            while snapshots.changed().await.is_ok() {
                let Some(window) = window.upgrade() else {
                    break;
                };
                let snapshot = snapshots.borrow_and_update().clone();
                window.show_torrents(&snapshot);
            }
        })?;
        self.snapshots.replace(Some(latest));
        Ok(())
    }

    /// Shows the window and runs its event loop.
    ///
    /// # Errors
    ///
    /// Returns [`RunError`] if showing the window or running its event loop fails.
    pub fn run(&self) -> Result<(), RunError> {
        self.component.run()?;
        // A watch receiver on another thread can request close before the follow task runs.
        if let Some(snapshots) = self.snapshots.borrow().as_ref() {
            let snapshot = snapshots.borrow().clone();
            self.show_torrents(&snapshot);
        }
        Ok(())
    }
}

#[expect(
    clippy::cast_precision_loss,
    reason = "Slint widths use f32 and logical column widths are small"
)]
fn visible_columns(registry: Vec<Column>, layout: &TableLayout) -> (Vec<ColumnSpec>, Vec<Column>) {
    let registry: HashMap<_, _> = registry
        .into_iter()
        .map(|column| (column.id, column))
        .collect();
    layout
        .columns
        .iter()
        .filter(|entry| entry.visible)
        .filter_map(|entry| {
            let column = *registry.get(&entry.id)?;
            let spec = ColumnSpec {
                id: column.id.as_str().into(),
                width: entry.width as f32,
                align: match column.align {
                    Align::Start => fluent::CellAlign::Start,
                    Align::End => fluent::CellAlign::End,
                },
                kind: match column.kind {
                    CellKind::Text => fluent::CellKind::Text,
                    CellKind::Progress => fluent::CellKind::Progress,
                },
            };
            Some((spec, column))
        })
        .unzip()
}

#[expect(
    clippy::cast_possible_truncation,
    reason = "The progress bar only needs Slint's f32 display precision"
)]
fn format_cell(value: &CellValue, text: &mut String) -> f32 {
    text.clear();
    write!(text, "{value}").expect("formatting a cell into a String cannot fail");
    match value {
        CellValue::Progress(fraction) => fraction.clamp(0.0, 1.0) as f32,
        _ => 0.0,
    }
}

fn make_cell(text: &str, fraction: f32) -> Cell {
    Cell {
        text: text.into(),
        fraction,
    }
}

#[cfg(test)]
mod tests {
    use i_slint_backend_testing::{AccessibleRole, ElementQuery};
    use slint::SharedString;
    use torrentide_core::{TorrentId, TorrentState, TorrentStatus};

    use super::*;
    use crate::columns::{self, ColumnId, ColumnLayout};

    fn window(columns: Vec<Column>) -> Window {
        i_slint_backend_testing::init_no_event_loop();
        Window::new(columns).expect("create the window with the testing backend")
    }

    fn headers(window: &Window) -> Vec<SharedString> {
        window
            .component
            .show()
            .expect("lay out the window before querying it");

        let mut headers = ElementQuery::from_root(&window.component)
            .match_accessible_role(AccessibleRole::Button)
            .find_all();
        headers.sort_by(|left, right| {
            left.absolute_position()
                .x
                .total_cmp(&right.absolute_position().x)
        });
        headers
            .iter()
            .map(|header| {
                header
                    .accessible_label()
                    .expect("column header has a label")
            })
            .collect()
    }

    fn torrent(name: &str) -> Torrent {
        Torrent {
            status: TorrentStatus {
                id: TorrentId::from_info_hash([0xab; 20]),
                name: Some(name.into()),
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

    fn displayed_row(window: &Window, index: usize) -> TorrentRow {
        window
            .component
            .get_rows()
            .row_data(index)
            .expect("the row is present in the window model")
    }

    #[test]
    fn window_shows_column_headers_in_order() {
        assert_eq!(
            headers(&window(columns::builtin())),
            ["Name", "Size", "Progress", "Down", "Up", "Ratio", "State"],
            "column headers should appear in left-to-right order"
        );
    }

    #[test]
    fn window_hides_columns_and_labels_custom_columns_by_id() {
        let mut columns = columns::builtin();
        columns[0].default_visible = false;
        columns[1].id = ColumnId::new("custom-size");
        assert_eq!(
            headers(&window(columns)),
            ["custom-size", "Progress", "Down", "Up", "Ratio", "State"]
        );
    }

    #[test]
    fn layout_entries_missing_from_the_registry_are_skipped() {
        let columns = columns::builtin();
        let mut layout = TableLayout::new(&columns);
        layout.columns.insert(
            1,
            ColumnLayout {
                id: ColumnId::new("missing"),
                width: 90,
                visible: true,
            },
        );
        assert_eq!(
            visible_columns(columns, &layout)
                .0
                .iter()
                .map(|spec| spec.id.as_str())
                .collect::<Vec<_>>(),
            [
                "name",
                "size",
                "progress",
                "download_rate",
                "upload_rate",
                "ratio",
                "state"
            ]
        );
    }

    #[test]
    fn window_shows_formatted_cells_in_name_order() {
        let window = window(columns::builtin());
        let mut complete = torrent("Complete");
        complete.status.done = 4096;
        window.show_torrents(&[torrent("In progress"), complete]);

        let texts = window.cell_texts();
        assert_eq!(
            texts[1],
            [
                "In progress",
                "2.0 KiB",
                "50.0%",
                "4.0 KiB/s",
                "256 B/s",
                "1.25",
                "Downloading"
            ]
        );
        assert_eq!(texts[0][0], "Complete");
        assert_eq!(
            displayed_row(&window, 0)
                .cells
                .row_data(2)
                .expect("progress")
                .fraction
                .to_bits(),
            1.0_f32.to_bits()
        );
    }

    #[test]
    fn window_sorts_names_case_insensitively() {
        let window = window(columns::builtin());
        window.show_torrents(&[torrent("beta"), torrent("Alpha"), torrent("gamma")]);
        let texts = window.cell_texts();
        let names: Vec<_> = texts.iter().map(|row| row[0].as_str()).collect();
        assert_eq!(names, ["Alpha", "beta", "gamma"]);
    }

    #[test]
    fn window_keeps_snapshot_order_when_sort_column_is_hidden() {
        let mut columns = columns::builtin();
        columns[0].default_visible = false;
        let window = window(columns);
        let mut larger = torrent("beta");
        larger.status.size = 3072;
        window.show_torrents(&[larger, torrent("Alpha")]);
        let texts = window.cell_texts();
        assert_eq!(texts[0][0], "3.0 KiB");
        assert_eq!(texts[1][0], "2.0 KiB");
    }

    #[test]
    fn window_follows_snapshot_updates() {
        i_slint_backend_testing::init_integration_test_with_mock_time();
        let window = Rc::new(Window::new(columns::builtin()).unwrap());
        let (publisher, snapshots) = watch::channel(vec![torrent("Initial")]);
        window.follow(snapshots).unwrap();
        assert_eq!(window.cell_texts()[0][0], "Initial");

        // Poll the task once to register its watch waker before publishing the next snapshot.
        slint::quit_event_loop().unwrap();
        slint::run_event_loop().unwrap();
        publisher.send(vec![torrent("Updated")]).unwrap();
        drop(publisher);
        slint::quit_event_loop().unwrap();
        slint::run_event_loop().unwrap();
        assert_eq!(window.cell_texts()[0][0], "Updated");
    }

    #[test]
    fn closer_ends_the_window_loop_from_another_thread() {
        i_slint_backend_testing::init_integration_test_with_system_time();
        let window = Window::new(columns::builtin()).unwrap();
        let closer = window.closer();
        let timed_out = Rc::new(std::cell::Cell::new(false));
        let watchdog = Rc::clone(&timed_out);
        slint::Timer::single_shot(std::time::Duration::from_secs(5), move || {
            watchdog.set(true);
            slint::quit_event_loop().unwrap();
        });

        std::thread::spawn(move || closer.close())
            .join()
            .expect("close thread did not panic")
            .expect("close request was queued");
        window.run().unwrap();
        assert!(!timed_out.get(), "close request did not end the event loop");
    }

    #[test]
    fn changed_value_updates_one_cell_without_replacing_rows() {
        let window = window(columns::builtin());
        let mut torrents = [torrent("First"), torrent("Second")];
        window.show_torrents(&torrents);
        let first = displayed_row(&window, 0).cells;
        let second = displayed_row(&window, 1).cells;
        let mut expected = first.iter().collect::<Vec<_>>();
        let unaffected = second.iter().collect::<Vec<_>>();

        torrents[0].status.download_rate = 8192;
        window.show_torrents(&torrents);

        expected[3].text = "8.0 KiB/s".into();
        assert!(
            first == displayed_row(&window, 0).cells,
            "first row model was replaced"
        );
        assert!(
            second == displayed_row(&window, 1).cells,
            "second row model was replaced"
        );
        assert_eq!(first.iter().collect::<Vec<_>>(), expected);
        assert_eq!(second.iter().collect::<Vec<_>>(), unaffected);
    }

    #[test]
    fn shorter_snapshot_removes_surplus_rows() {
        let window = window(columns::builtin());
        let torrents = [torrent("First"), torrent("Second")];
        window.show_torrents(&torrents);
        window.show_torrents(&torrents[..1]);
        assert_eq!(window.component.get_rows().row_count(), 1);
        window.show_torrents(&[]);
        assert_eq!(window.component.get_rows().row_count(), 0);
    }
}
