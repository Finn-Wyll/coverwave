//! Shared, platform-agnostic types for coverwave.
//!
//! Nothing in this crate may depend on a terminal, OS audio, the network or
//! the filesystem: it is shared with future non-terminal frontends.

pub mod art;
pub mod audio;
pub mod color;
pub mod queue;
pub mod recipe;
pub use art::{ArtStyle, Palette, RgbImage};
pub use audio::{AudioFrame, BAND_COUNT};
pub use color::{Lab, ansi256};
pub use queue::{Queue, Repeat};
pub use recipe::{Layer, Recipe};

/// A 24-bit colour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    /// Linear blend from `self` (t = 0) to `other` (t = 1).
    pub fn lerp(self, other: Rgb, t: f32) -> Rgb {
        let t = t.clamp(0.0, 1.0);
        let mix = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
        Rgb(
            mix(self.0, other.0),
            mix(self.1, other.1),
            mix(self.2, other.2),
        )
    }
}

/// One terminal cell: a glyph plus colours. `bg: None` means "leave the
/// frontend's background alone".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Cell {
    pub ch: char,
    pub fg: Rgb,
    pub bg: Option<Rgb>,
}

impl Cell {
    pub const BLANK: Cell = Cell {
        ch: ' ',
        fg: Rgb(0, 0, 0),
        bg: None,
    };
}

impl Default for Cell {
    fn default() -> Self {
        Cell::BLANK
    }
}

/// A row-major grid of cells: the renderer's output and the frontend's input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CellGrid {
    width: u16,
    height: u16,
    cells: Vec<Cell>,
}

impl CellGrid {
    pub fn new(width: u16, height: u16) -> Self {
        CellGrid {
            width,
            height,
            cells: vec![Cell::BLANK; width as usize * height as usize],
        }
    }

    pub fn width(&self) -> u16 {
        self.width
    }

    pub fn height(&self) -> u16 {
        self.height
    }

    fn index(&self, x: u16, y: u16) -> Option<usize> {
        (x < self.width && y < self.height).then(|| y as usize * self.width as usize + x as usize)
    }

    pub fn get(&self, x: u16, y: u16) -> Option<&Cell> {
        self.index(x, y).map(|i| &self.cells[i])
    }

    /// Writes a cell; out-of-bounds writes are ignored.
    pub fn set(&mut self, x: u16, y: u16, cell: Cell) {
        if let Some(i) = self.index(x, y) {
            self.cells[i] = cell;
        }
    }

    pub fn cells(&self) -> &[Cell] {
        &self.cells
    }

    /// Number of cells that differ from `other`. Grids of different sizes
    /// count as fully changed.
    pub fn diff_count(&self, other: &CellGrid) -> usize {
        if self.width != other.width || self.height != other.height {
            return self.cells.len();
        }
        self.cells
            .iter()
            .zip(&other.cells)
            .filter(|(a, b)| a != b)
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_get_and_bounds() {
        let mut g = CellGrid::new(3, 2);
        let c = Cell {
            ch: '#',
            fg: Rgb(1, 2, 3),
            bg: None,
        };
        g.set(2, 1, c);
        g.set(3, 0, c); // ignored
        assert_eq!(g.get(2, 1), Some(&c));
        assert_eq!(g.get(3, 0), None);
        assert_eq!(g.diff_count(&CellGrid::new(3, 2)), 1);
    }

    #[test]
    fn lerp_endpoints() {
        let a = Rgb(0, 100, 200);
        let b = Rgb(200, 100, 0);
        assert_eq!(a.lerp(b, 0.0), a);
        assert_eq!(a.lerp(b, 1.0), b);
        assert_eq!(a.lerp(b, 0.5), Rgb(100, 100, 100));
    }
}
