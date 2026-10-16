//! GFM table editing: cell lookup, structural changes, and serialization.
//!
//! A table's lines are: header row, delimiter row, then body rows. "Grid rows" skip
//! the delimiter: grid row 0 is the header and grid row `r >= 1` is line `r + 1`.

use std::ops::Range;

use crate::markdown::{Align, delim_row, split_cells};

/// Table actions offered by the right-click menu and the command palette.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TableCmd {
    AlignLeft,
    AlignCenter,
    AlignRight,
    RowAbove,
    RowBelow,
    DeleteRow,
    ColumnLeft,
    ColumnRight,
    DeleteColumn,
    DeleteTable,
}

impl TableCmd {
    /// Menu order and labels.
    pub const ALL: [(TableCmd, &'static str); 10] = [
        (TableCmd::AlignLeft, "Align Column Left"),
        (TableCmd::AlignCenter, "Align Column Center"),
        (TableCmd::AlignRight, "Align Column Right"),
        (TableCmd::RowAbove, "Insert Row Above"),
        (TableCmd::RowBelow, "Insert Row Below"),
        (TableCmd::DeleteRow, "Delete Row"),
        (TableCmd::ColumnLeft, "Insert Column Left"),
        (TableCmd::ColumnRight, "Insert Column Right"),
        (TableCmd::DeleteColumn, "Delete Column"),
        (TableCmd::DeleteTable, "Delete Table"),
    ];

    pub fn align(self) -> Option<Align> {
        match self {
            TableCmd::AlignLeft => Some(Align::Left),
            TableCmd::AlignCenter => Some(Align::Center),
            TableCmd::AlignRight => Some(Align::Right),
            _ => None,
        }
    }
}

/// Table line offset of grid row `r`.
pub fn line_of_row(r: usize) -> usize {
    if r == 0 { 0 } else { r + 1 }
}

/// Grid row of table line offset `l`, or None for the delimiter row.
pub fn row_of_line(l: usize) -> Option<usize> {
    match l {
        0 => Some(0),
        1 => None,
        _ => Some(l - 1),
    }
}

/// Index of the cell containing byte `col`, or the nearest cell when `col` falls on
/// padding or a pipe between cells.
pub fn cell_index(cells: &[Range<usize>], col: usize) -> usize {
    let dist = |r: &Range<usize>| {
        if col < r.start {
            r.start - col
        } else {
            col.saturating_sub(r.end)
        }
    };
    (0..cells.len())
        .min_by_key(|&i| dist(&cells[i]))
        .unwrap_or(0)
}

/// A table parsed into cell texts (source text, trimmed, escapes kept).
#[derive(Clone, PartialEq, Debug)]
pub struct Grid {
    /// Blockquote/indent prefix repeated on every line.
    pub prefix: String,
    pub aligns: Vec<Align>,
    /// Grid rows: header first, delimiter row excluded. Every row has `cols()` cells.
    pub rows: Vec<Vec<String>>,
}

impl Grid {
    /// Parses a table's lines (delimiter row second); `qend` is the prefix length.
    pub fn parse(lines: &[&str], qend: usize) -> Grid {
        let texts = |line: &str| -> Vec<String> {
            split_cells(line, qend.min(line.len()))
                .into_iter()
                .map(|r| line[r].to_string())
                .collect()
        };
        let header = texts(lines[0]);
        let cols = header.len().max(1);
        let fit = |mut row: Vec<String>| {
            row.resize(cols, String::new());
            row
        };
        let mut aligns = lines
            .get(1)
            .and_then(|l| delim_row(&l[qend.min(l.len())..]))
            .unwrap_or_default();
        aligns.resize(cols, Align::Left);
        let mut rows = vec![fit(header)];
        rows.extend(lines.iter().skip(2).map(|l| fit(texts(l))));
        Grid {
            prefix: lines[0][..qend.min(lines[0].len())].to_string(),
            aligns,
            rows,
        }
    }

    /// A new table with a header row and one empty body row.
    pub fn new(prefix: &str, headers: &[&str]) -> Grid {
        Grid {
            prefix: prefix.to_string(),
            aligns: vec![Align::Left; headers.len()],
            rows: vec![
                headers.iter().map(|h| h.to_string()).collect(),
                vec![String::new(); headers.len()],
            ],
        }
    }

    pub fn cols(&self) -> usize {
        self.aligns.len()
    }

    /// Serializes with columns padded to equal width.
    pub fn to_lines(&self) -> Vec<String> {
        let widths: Vec<usize> = (0..self.cols())
            .map(|c| {
                self.rows
                    .iter()
                    .map(|r| r[c].chars().count())
                    .max()
                    .unwrap_or(0)
                    .max(3)
            })
            .collect();
        let row_line = |cells: &[String]| {
            let mut s = format!("{}|", self.prefix);
            for (c, text) in cells.iter().enumerate() {
                let pad = widths[c] - text.chars().count();
                s.push_str(&format!(" {text}{} |", " ".repeat(pad)));
            }
            s
        };
        let mut delim = format!("{}|", self.prefix);
        for (c, a) in self.aligns.iter().enumerate() {
            let w = widths[c];
            let dashes = match a {
                Align::Left => "-".repeat(w),
                Align::Center => format!(":{}:", "-".repeat(w - 2)),
                Align::Right => format!("{}:", "-".repeat(w - 1)),
            };
            delim.push_str(&format!(" {dashes} |"));
        }
        let mut out = vec![row_line(&self.rows[0]), delim];
        out.extend(self.rows[1..].iter().map(|r| row_line(r)));
        out
    }

    /// Inserts an empty body row at grid row `at` (at least 1, below the header).
    pub fn insert_row(&mut self, at: usize) {
        let at = at.clamp(1, self.rows.len());
        self.rows.insert(at, vec![String::new(); self.cols()]);
    }

    /// Deletes body row `r`; the header row cannot be deleted.
    pub fn delete_row(&mut self, r: usize) -> bool {
        if r == 0 || r >= self.rows.len() {
            return false;
        }
        self.rows.remove(r);
        true
    }

    pub fn insert_col(&mut self, at: usize) {
        let at = at.min(self.cols());
        self.aligns.insert(at, Align::Left);
        for row in &mut self.rows {
            row.insert(at, String::new());
        }
    }

    /// Deletes column `c` unless it is the only one.
    pub fn delete_col(&mut self, c: usize) -> bool {
        if self.cols() <= 1 || c >= self.cols() {
            return false;
        }
        self.aligns.remove(c);
        for row in &mut self.rows {
            row.remove(c);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid(src: &str) -> Grid {
        let lines: Vec<&str> = src.lines().collect();
        Grid::parse(&lines, 0)
    }

    #[test]
    fn parse_and_serialize() {
        let g = grid("| Name | Qty |\n|:--|--:|\n| apple | 3 |\n| a \\| b |");
        assert_eq!(g.aligns, vec![Align::Left, Align::Right]);
        assert_eq!(g.rows[1], vec!["apple", "3"]);
        // Short rows are padded; escaped pipes stay escaped.
        assert_eq!(g.rows[2], vec!["a \\| b", ""]);
        assert_eq!(
            g.to_lines(),
            vec![
                "| Name   | Qty |",
                "| ------ | --: |",
                "| apple  | 3   |",
                "| a \\| b |     |",
            ]
        );
        assert_eq!(grid(&g.to_lines().join("\n")), g);
    }

    #[test]
    fn structure_changes() {
        let mut g = Grid::new("> ", &["A", "B"]);
        assert_eq!(
            g.to_lines(),
            vec!["> | A   | B   |", "> | --- | --- |", "> |     |     |"]
        );
        g.insert_row(1);
        assert_eq!(g.rows.len(), 3);
        assert!(!g.delete_row(0));
        assert!(g.delete_row(2));
        g.insert_col(1);
        g.aligns[1] = Align::Center;
        assert_eq!(g.rows[0], vec!["A", "", "B"]);
        assert_eq!(g.to_lines()[1], "> | --- | :-: | --- |");
        assert!(g.delete_col(0) && g.delete_col(0));
        assert!(!g.delete_col(0));
        assert_eq!(
            Grid::parse(
                &g.to_lines().iter().map(String::as_str).collect::<Vec<_>>(),
                2
            ),
            g
        );
    }

    #[test]
    fn cells_and_rows() {
        let cells = split_cells("| ab | cd |", 0);
        assert_eq!(cell_index(&cells, 2), 0);
        assert_eq!(cell_index(&cells, 5), 0);
        assert_eq!(cell_index(&cells, 6), 1);
        assert_eq!(cell_index(&cells, 11), 1);
        assert_eq!((line_of_row(0), line_of_row(2)), (0, 3));
        assert_eq!(
            (row_of_line(0), row_of_line(1), row_of_line(3)),
            (Some(0), None, Some(2))
        );
    }
}
