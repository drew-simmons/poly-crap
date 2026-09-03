//! Whitespace-aligned tables for human output.
//!
//! A cell is padded to the widest text in its column before any style is
//! applied, so escape codes never count toward a width. The last column is
//! never padded and trailing whitespace is trimmed, so a row whose final cell
//! is empty ends where its text does.

use crate::style::{HEADER, Theme};
use anstyle::Style;
use std::fmt::Write as _;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Align {
    Left,
    Right,
}

#[derive(Debug, Clone, Copy)]
pub struct Column {
    pub title: &'static str,
    pub align: Align,
}

impl Column {
    #[must_use]
    pub const fn left(title: &'static str) -> Self {
        Self {
            title,
            align: Align::Left,
        }
    }

    #[must_use]
    pub const fn right(title: &'static str) -> Self {
        Self {
            title,
            align: Align::Right,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Cell {
    pub text: String,
    pub style: Style,
}

impl Cell {
    #[must_use]
    pub fn plain(text: impl Into<String>) -> Self {
        Self::styled(text, Style::new())
    }

    #[must_use]
    pub fn styled(text: impl Into<String>, style: Style) -> Self {
        Self {
            text: text.into(),
            style,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Table {
    columns: Vec<Column>,
    rows: Vec<Vec<Cell>>,
}

impl Table {
    #[must_use]
    pub const fn new(columns: Vec<Column>) -> Self {
        Self {
            columns,
            rows: Vec::new(),
        }
    }

    /// Add a row. It must hold one cell per column.
    pub fn push(&mut self, row: Vec<Cell>) {
        assert_eq!(
            row.len(),
            self.columns.len(),
            "row width must match the columns"
        );
        self.rows.push(row);
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// Render the header and every row, each line prefixed with `indent`.
    #[must_use]
    pub fn render(&self, theme: Theme, indent: &str) -> String {
        let widths = self.widths();
        let header: Vec<Cell> = self
            .columns
            .iter()
            .map(|column| Cell::styled(column.title, HEADER))
            .collect();
        let mut output = String::new();
        for row in std::iter::once(&header).chain(&self.rows) {
            self.render_line(&mut output, row, &widths, theme, indent);
        }
        output
    }

    fn widths(&self) -> Vec<usize> {
        self.columns
            .iter()
            .enumerate()
            .map(|(index, column)| {
                self.rows
                    .iter()
                    .map(|row| width(&row[index].text))
                    .fold(width(column.title), usize::max)
            })
            .collect()
    }

    fn render_line(
        &self,
        output: &mut String,
        row: &[Cell],
        widths: &[usize],
        theme: Theme,
        indent: &str,
    ) {
        let last = row.len().saturating_sub(1);
        let cells: Vec<String> = row
            .iter()
            .zip(&self.columns)
            .zip(widths)
            .enumerate()
            .map(|(index, ((cell, column), width))| {
                let fitted = fit(&cell.text, column.align, (index < last).then_some(*width));
                theme.paint(cell.style, fitted)
            })
            .collect();
        let line = format!("{indent}{}", cells.join("  "));
        writeln!(output, "{}", line.trim_end()).expect("writing to a String cannot fail");
    }
}

fn width(text: &str) -> usize {
    text.chars().count()
}

/// Pad text to a width, or leave it alone when there is none: the last column
/// never pads, so a line ends where its text does.
fn fit(text: &str, align: Align, width: Option<usize>) -> String {
    match (width, align) {
        (None, _) => text.to_string(),
        (Some(width), Align::Left) => format!("{text:<width$}"),
        (Some(width), Align::Right) => format!("{text:>width$}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::style::BAD;

    fn table() -> Table {
        let mut table = Table::new(vec![
            Column::right("N"),
            Column::left("Name"),
            Column::left("Note"),
        ]);
        table.push(vec![
            Cell::plain("1.0"),
            Cell::plain("run"),
            Cell::plain(""),
        ]);
        table.push(vec![
            Cell::styled("12.5", BAD),
            Cell::plain("longer_name"),
            Cell::plain("5-6"),
        ]);
        table
    }

    #[test]
    fn columns_align_to_the_widest_text() {
        let rendered = table().render(Theme::plain(), "  ");
        let lines: Vec<_> = rendered.lines().collect();
        assert_eq!(
            lines,
            [
                "     N  Name         Note",
                "   1.0  run",
                "  12.5  longer_name  5-6",
            ]
        );
        assert!(rendered.ends_with('\n'));
    }

    #[test]
    fn styles_wrap_the_padded_cell() {
        let rendered = table().render(Theme::ansi(), "");
        let lines: Vec<_> = rendered.lines().collect();
        // The header is bold; the score is padded inside its own codes.
        assert!(lines[0].starts_with("\x1b["), "{:?}", lines[0]);
        assert!(lines[0].contains("   N\x1b[0m  "), "{:?}", lines[0]);
        assert!(
            lines[2].contains("12.5\x1b[0m  longer_name"),
            "{:?}",
            lines[2]
        );
        // Plain cells stay bare, so an empty last cell leaves nothing behind.
        assert!(lines[1].ends_with("run"), "{:?}", lines[1]);
    }

    #[test]
    fn fit_pads_every_column_but_the_last() {
        assert_eq!(fit("ab", Align::Left, Some(4)), "ab  ");
        assert_eq!(fit("ab", Align::Right, Some(4)), "  ab");
        assert_eq!(fit("ab", Align::Right, None), "ab");
        assert_eq!(width("héllo"), 5);
    }

    #[test]
    fn an_empty_table_is_only_a_header() {
        let table = Table::new(vec![Column::left("A"), Column::right("B")]);
        assert!(table.is_empty());
        assert_eq!(table.render(Theme::plain(), ""), "A  B\n");
    }

    #[test]
    #[should_panic(expected = "row width")]
    fn a_short_row_is_rejected() {
        table().push(vec![Cell::plain("only one")]);
    }
}
