//! A single character cell in the terminal grid.

/// Colour as expressed by the remote side. Resolved to a concrete RGB value by
/// the active theme's palette (see [`crate::term::palette`]), so a theme switch
/// recolours existing scrollback without re-parsing anything.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Color {
    /// Whatever the theme calls foreground/background.
    #[default]
    Default,
    /// One of the 256 indexed colours; 0..16 are the ANSI set.
    Indexed(u8),
    /// 24-bit colour from an SGR 38;2;r;g;b sequence.
    Rgb(u8, u8, u8),
}

/// Rendition attributes carried by a cell, as a small bitset.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Attrs(u8);

impl Attrs {
    pub const EMPTY: Self = Attrs(0);
    pub const BOLD: Self = Attrs(1 << 0);
    pub const DIM: Self = Attrs(1 << 1);
    pub const ITALIC: Self = Attrs(1 << 2);
    pub const UNDERLINE: Self = Attrs(1 << 3);
    pub const BLINK: Self = Attrs(1 << 4);
    pub const REVERSE: Self = Attrs(1 << 5);
    pub const HIDDEN: Self = Attrs(1 << 6);
    pub const STRIKE: Self = Attrs(1 << 7);

    #[inline]
    pub fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    #[inline]
    pub fn insert(&mut self, other: Self) {
        self.0 |= other.0;
    }

    #[inline]
    pub fn remove(&mut self, other: Self) {
        self.0 &= !other.0;
    }

    #[inline]
    pub fn is_empty(self) -> bool {
        self.0 == 0
    }
}

/// The rendition state the parser applies to every printed character. Kept
/// separate from [`Cell`] so SGR handling mutates one small value.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Pen {
    pub fg: Color,
    pub bg: Color,
    pub attrs: Attrs,
}

impl Pen {
    /// SGR 0 — back to the theme defaults with no attributes.
    pub fn reset(&mut self) {
        *self = Pen::default();
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    pub ch: char,
    pub fg: Color,
    pub bg: Color,
    pub attrs: Attrs,
}

impl Default for Cell {
    fn default() -> Self {
        Cell {
            ch: ' ',
            fg: Color::Default,
            bg: Color::Default,
            attrs: Attrs::EMPTY,
        }
    }
}

impl Cell {
    /// A blank cell that still carries the current background, which is what
    /// erase operations (ED/EL) are expected to leave behind.
    pub fn blank(pen: &Pen) -> Self {
        Cell {
            ch: ' ',
            fg: pen.fg,
            bg: pen.bg,
            attrs: Attrs::EMPTY,
        }
    }

    pub fn with_pen(ch: char, pen: &Pen) -> Self {
        Cell {
            ch,
            fg: pen.fg,
            bg: pen.bg,
            attrs: pen.attrs,
        }
    }

    pub fn is_blank(&self) -> bool {
        self.ch == ' ' || self.ch == '\0'
    }
}
