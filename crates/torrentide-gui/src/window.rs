//! Adapts the column registry and torrent snapshots to the Slint table.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fmt::{self, Write as _};
use std::rc::Rc;

use slint::{ComponentHandle, Model, ModelRc, PlatformError, VecModel};
use torrentide_core::Torrent;

use crate::columns::{Align, CellKind, CellValue, Column, TableLayout};
use crate::fluent::{self, Cell, ColumnSpec, MainWindow, TorrentRow};

/// The window system could not be started or failed while running.
#[derive(Debug, thiserror::Error)]
#[error("the window system failed")]
pub struct RunError(#[from] PlatformError);

/// Displays torrent snapshots in a single-threaded Slint window.
pub struct Window {
    component: MainWindow,
    columns: Vec<Column>,
    rows: Rc<VecModel<TorrentRow>>,
    shown: RefCell<Vec<ShownRow>>,
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
            rows,
            shown: RefCell::default(),
        })
    }

    /// Displays the torrents in slice order, updating only cells whose display changes.
    pub fn show_torrents(&self, torrents: &[Torrent]) {
        let mut shown = self.shown.borrow_mut();
        while shown.len() > torrents.len() {
            self.rows.remove(shown.len() - 1);
            shown.pop();
        }

        let mut text = String::new();
        for (row, torrent) in shown.iter_mut().zip(torrents) {
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
        }

        let new_torrents = &torrents[shown.len()..];
        shown.reserve(new_torrents.len());
        for torrent in new_torrents {
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

    /// Shows the window and runs its event loop.
    ///
    /// # Errors
    ///
    /// Returns [`RunError`] if showing the window or running its event loop fails.
    pub fn run(&self) -> Result<(), RunError> {
        self.component.run()?;
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
    fn window_shows_formatted_cells_in_snapshot_order() {
        let window = window(columns::builtin());
        let mut complete = torrent("Complete");
        complete.status.done = 4096;
        window.show_torrents(&[torrent("In progress"), complete]);

        let first = displayed_row(&window, 0).cells;
        let second = displayed_row(&window, 1).cells;
        assert_eq!(
            first.iter().map(|cell| cell.text).collect::<Vec<_>>(),
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
        assert_eq!(second.row_data(0).expect("name").text, "Complete");
        assert_eq!(
            second.row_data(2).expect("progress").fraction.to_bits(),
            1.0_f32.to_bits()
        );
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
