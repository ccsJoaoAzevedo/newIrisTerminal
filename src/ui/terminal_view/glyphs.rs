//! Laying characters out once, and drawing a row of them as one shape.
//!
//! The grid's hot path. A screen is tens of thousands of characters and is
//! redrawn whole every frame, so both halves of the cost are cached: the
//! layout of each distinct character in [`GlyphCache`], and the buffers a row
//! is assembled in, in [`Scratch`].

use super::*;

/// One laid-out glyph per distinct character, reused for every cell that shows
/// it and for every frame until the font changes.
///
/// A screen of text is tens of thousands of characters, and asking egui to lay
/// each one out costs a `String`, a layout job and a hash lookup *per cell, per
/// frame* - which at sixty frames a second was the single most expensive thing
/// the terminal did. The glyphs themselves are identical: one character, one
/// monospace font, always at the same size. So they are laid out once.
///
/// Colour is deliberately not part of the key. A galley laid out in
/// [`Color32::PLACEHOLDER`] takes whatever colour the shape that draws it
/// carries, so the same cached glyph serves every colour on screen.
#[derive(Default)]
pub(super) struct GlyphCache {
    /// What the entries were built for: the installed faces, the font asked
    /// for, and the DPI scale the glyph meshes were snapped to. Any of the
    /// three changing makes every entry wrong, so all of them go at once.
    pub(super) key: Option<(u64, FontId, u32)>,
    /// Printable ASCII, indexed by the character itself. An IRIS session is
    /// very nearly all of it, and a bounds-checked index beats hashing.
    pub(super) ascii: Vec<Option<std::sync::Arc<egui::Galley>>>,
    /// Everything else - accented Portuguese, box drawing, the occasional
    /// symbol - rare enough to pay for a hash.
    pub(super) other: std::collections::HashMap<char, std::sync::Arc<egui::Galley>>,
}

impl GlyphCache {
    /// Throws the entries away if they were built for anything but what is
    /// about to be drawn.
    ///
    /// Once a frame, deliberately: reading the DPI scale takes a lock inside
    /// egui, and paying that per character is most of what laying each cell out
    /// used to cost.
    pub(super) fn prepare(&mut self, ctx: &egui::Context, font: &FontId) {
        let key = (
            crate::ui::fonts::generation(),
            font.clone(),
            ctx.pixels_per_point().to_bits(),
        );
        if self.key.as_ref() != Some(&key) {
            self.ascii.clear();
            self.ascii.resize(128, None);
            self.other.clear();
            self.key = Some(key);
        }
    }

    /// The galley for one character, laid out at most once per font.
    ///
    /// The per-cell path, and on all but the first sighting of a character
    /// nothing but an index. Takes `&mut self` rather than reaching for the
    /// thread-local itself: that lookup is cheap once and was not cheap tens of
    /// thousands of times a frame.
    pub(super) fn get(
        &mut self,
        ctx: &egui::Context,
        ch: char,
        font: &FontId,
    ) -> std::sync::Arc<egui::Galley> {
        let ascii = (ch as usize) < self.ascii.len();
        if ascii {
            if let Some(galley) = &self.ascii[ch as usize] {
                return std::sync::Arc::clone(galley);
            }
        } else if let Some(galley) = self.other.get(&ch) {
            return std::sync::Arc::clone(galley);
        }
        // PLACEHOLDER, so the colour comes from the shape and one entry covers
        // every colour the character is ever drawn in.
        let galley =
            ctx.fonts(|f| f.layout_no_wrap(ch.to_string(), font.clone(), Color32::PLACEHOLDER));
        if ascii {
            self.ascii[ch as usize] = Some(std::sync::Arc::clone(&galley));
        } else {
            self.other.insert(ch, std::sync::Arc::clone(&galley));
        }
        galley
    }
}

/// Assembles one display row's glyphs into a single shape.
///
/// egui charges per shape, not per glyph: for every `Shape::Text` the
/// tessellator computes a rotation matrix - two transcendental calls, for an
/// angle that is always zero here - and reserves room in the output mesh of its
/// own accord. At one shape per character that was ~9,500 of each per frame,
/// and it showed up as `sincosf` and as reallocation inside the Windows heap.
///
/// A row is one shape instead. The glyph meshes egui already laid out are
/// copied into a single mesh, each at the lattice position its cell sits on,
/// and handed over as a [`Galley`] of one row rather than as a raw mesh - which
/// is what keeps this safe. A galley's texture coordinates stay in texels and
/// are scaled by the tessellator against whatever the font atlas measures at
/// the moment of drawing; building a mesh directly would mean baking that scale
/// in at the wrong moment and drawing a frame of garbage every time the atlas
/// grew.
///
/// [`Galley`]: egui::Galley
pub(super) struct RowMesh {
    pub(super) mesh: egui::epaint::Mesh,
    pub(super) bounds: Rect,
    /// The empty layout job every row's galley points at. A galley whose job
    /// has no sections reports itself empty and is skipped by the tessellator,
    /// so there has to be one - and it is the same for every row, so it is made
    /// once a frame rather than once a row.
    pub(super) job: std::sync::Arc<egui::text::LayoutJob>,
    /// Pixels per point, when the tessellator is snapping text to the pixel
    /// grid, which is its default. `None` when it is not.
    ///
    /// Each glyph used to be its own shape, and so was snapped to the pixel
    /// grid on its own. A row is one shape, snapped once - so the snapping of
    /// each character has to happen here instead, and it has to give the same
    /// answer to the last bit. It does, because the row's shape is placed at
    /// x = 0 and every glyph carries its whole snapped x: the tessellator then
    /// adds a zero, where measuring each glyph against a snapped row origin
    /// would have added and subtracted it and lost a few bits doing so. At
    /// 1.25x that was already visible in a test.
    pub(super) snap: Option<f32>,
}

impl RowMesh {
    pub(super) fn new(ctx: &egui::Context) -> Self {
        let job = egui::text::LayoutJob {
            sections: vec![egui::text::LayoutSection {
                leading_space: 0.0,
                byte_range: 0..0,
                format: egui::text::TextFormat::default(),
            }],
            ..Default::default()
        };
        Self {
            mesh: egui::epaint::Mesh::default(),
            bounds: Rect::NOTHING,
            job: std::sync::Arc::new(job),
            snap: ctx
                .options(|o| o.tessellation_options.round_text_to_pixels)
                .then(|| ctx.pixels_per_point()),
        }
    }

    /// Where a coordinate ends up once the tessellator has snapped it.
    pub(super) fn snapped(&self, x: f32) -> f32 {
        match self.snap {
            Some(ppp) => (x * ppp).round() / ppp,
            None => x,
        }
    }

    /// Makes room for a row of at most `glyphs` characters.
    ///
    /// The mesh is moved out of here into the row's galley, so each row starts
    /// with nothing and would otherwise grow itself a dozen times over - the
    /// same reallocation the shape buffer was costing before it was kept.
    pub(super) fn reserve(&mut self, glyphs: usize) {
        self.mesh.vertices.reserve(glyphs * 4);
        self.mesh.indices.reserve(glyphs * 6);
    }

    /// Adds one character at `x`, in the colour its cell calls for.
    ///
    /// `x` is the lattice position in the pane's own coordinates, and is stored
    /// snapped and whole - see `snap` for why it is not an offset.
    pub(super) fn push(&mut self, glyph: &egui::Galley, x: f32, colour: Color32) {
        let Some(row) = glyph.rows.first() else {
            return;
        };
        let source = &row.visuals.mesh;
        if source.vertices.is_empty() {
            return;
        }
        let dx = self.snapped(x);
        let base = u32::try_from(self.mesh.vertices.len()).unwrap_or(0);
        self.mesh
            .vertices
            .extend(source.vertices.iter().map(|vertex| {
                let pos = Pos2::new(vertex.pos.x + dx, vertex.pos.y);
                self.bounds.extend_with(pos);
                egui::epaint::Vertex {
                    pos,
                    uv: vertex.uv,
                    color: colour,
                }
            }));
        self.mesh
            .indices
            .extend(source.indices.iter().map(|index| index + base));
    }

    /// Hands the row over as one shape and empties the buffers for the next.
    ///
    /// Placed at x = 0, because the glyphs already carry their own x; only the
    /// row's y is left for the tessellator to snap, and every glyph on the row
    /// shares it. Nothing is produced for a row with no glyphs on it, which
    /// most of an idle screen is.
    pub(super) fn take(&mut self, y: f32, ppp: f32) -> Option<egui::Shape> {
        if self.mesh.vertices.is_empty() {
            return None;
        }
        let mesh = std::mem::take(&mut self.mesh);
        let bounds = std::mem::replace(&mut self.bounds, Rect::NOTHING);
        let vertices = mesh.vertices.len();
        let indices = mesh.indices.len();
        let galley = egui::Galley {
            job: std::sync::Arc::clone(&self.job),
            rows: vec![egui::epaint::text::Row {
                section_index_at_start: 0,
                // Only the tessellator reads this galley, and it reads the
                // mesh. The per-character data a laid-out galley carries is
                // for cursors and selection, which the grid does itself.
                glyphs: Vec::new(),
                rect: bounds,
                visuals: egui::epaint::text::RowVisuals {
                    mesh,
                    mesh_bounds: bounds,
                    glyph_vertex_range: 0..vertices,
                },
                ends_with_newline: false,
            }],
            elided: false,
            rect: bounds,
            mesh_bounds: bounds,
            num_vertices: vertices,
            num_indices: indices,
            pixels_per_point: ppp,
        };
        // Every vertex already carries its own colour, so the fallback is never
        // reached; it may not be PLACEHOLDER all the same.
        Some(egui::Shape::galley(
            Pos2::new(0.0, y),
            std::sync::Arc::new(galley),
            Color32::WHITE,
        ))
    }
}

/// What drawing the grid keeps from one frame to the next.
///
/// Both halves exist so that a frame does not begin by building what the last
/// one already had: the glyphs, and the buffer their shapes are collected in -
/// which grows to tens of thousands of entries, and was being grown from empty
/// every single frame.
#[derive(Default)]
pub(super) struct Scratch {
    pub(super) glyphs: GlyphCache,
    pub(super) shapes: Vec<egui::Shape>,
    /// Underlines and strikes, which are drawn over the glyphs and so cannot go
    /// into `shapes` until the row's own shape has.
    pub(super) marks: Vec<egui::Shape>,
}

thread_local! {
    /// egui is single-threaded and every pane draws in the same font, so one
    /// of these serves the whole window.
    pub(super) static SCRATCH: std::cell::RefCell<Scratch> = std::cell::RefCell::new(Scratch::default());
}

#[cfg(test)]
mod tests {
    use super::*;
    /// The whole safety argument for drawing a row as one shape: the pixels
    /// have to come out where one shape per glyph put them. Compared after
    /// tessellation, which is where the snapping and the texture coordinates
    /// are settled, so nothing about either path is taken on trust.
    #[test]
    fn a_row_drawn_as_one_shape_lands_exactly_where_one_shape_per_glyph_did() {
        // Including the display scalings where the two paths could disagree: a
        // row is snapped to the pixel grid once, at its origin, where before
        // every glyph was snapped on its own.
        for scale in [1.0_f32, 1.25, 1.5, 2.0] {
            same_pixels_at_scale(scale);
        }
    }
    fn same_pixels_at_scale(scale: f32) {
        let ctx = egui::Context::default();
        ctx.set_pixels_per_point(scale);
        let _ = ctx.run(egui::RawInput::default(), |_| {});
        let ppp = ctx.pixels_per_point();
        assert_eq!(ppp, scale, "the context did not take the scale");
        let font = FontId::monospace(14.0);
        let mut cache = GlyphCache::default();
        cache.prepare(&ctx, &font);
        let cell = ctx.fonts(|f| {
            Vec2::new(
                f.glyph_width(&font, 'M').ceil().max(1.0),
                f.row_height(&font).ceil().max(1.0),
            )
        });
        // Fractional, because a whole-pixel origin would hide any rounding
        // difference between the two paths.
        let left = 7.3;
        let y = 11.7;
        let text = "do ^%CSW1A  ç1";
        let colour = Color32::from_rgb(200, 30, 40);

        let per_glyph: Vec<egui::Shape> = text
            .chars()
            .enumerate()
            .map(|(col, ch)| {
                egui::Shape::galley(
                    Pos2::new(glyph_x(left, col, cell), y),
                    cache.get(&ctx, ch, &font),
                    colour,
                )
            })
            .collect();

        let mut row = RowMesh::new(&ctx);
        for (col, ch) in text.chars().enumerate() {
            let glyph = cache.get(&ctx, ch, &font);
            row.push(&glyph, glyph_x(left, col, cell), colour);
        }
        let as_one_row = vec![row.take(y, ppp).expect("the row has glyphs on it")];

        let clip = Rect::from_min_size(Pos2::ZERO, Vec2::new(2000.0, 2000.0));
        let tessellate = |shapes: Vec<egui::Shape>| {
            ctx.tessellate(
                shapes
                    .into_iter()
                    .map(|shape| egui::epaint::ClippedShape {
                        clip_rect: clip,
                        shape,
                    })
                    .collect(),
                ppp,
            )
            .into_iter()
            .flat_map(|part| match part.primitive {
                egui::epaint::Primitive::Mesh(mesh) => mesh.vertices,
                egui::epaint::Primitive::Callback(_) => Vec::new(),
            })
            .collect::<Vec<_>>()
        };

        let old = tessellate(per_glyph);
        let new = tessellate(as_one_row);
        assert!(!old.is_empty(), "at {scale}x the comparison drew nothing");
        assert_eq!(
            old.len(),
            new.len(),
            "at {scale}x: a different number of vertices"
        );
        for (i, (old, new)) in old.iter().zip(&new).enumerate() {
            assert_eq!(old, new, "at {scale}x: vertex {i} moved");
        }
    }
    #[test]
    fn the_glyph_cache_reuses_a_layout_and_throws_it_away_when_the_font_changes() {
        let ctx = egui::Context::default();
        // Fonts only exist inside a frame, so give it one.
        let _ = ctx.run(egui::RawInput::default(), |_| {});

        let mut cache = GlyphCache::default();
        let small = FontId::monospace(14.0);
        cache.prepare(&ctx, &small);
        // ASCII and not, because the two take different paths through the cache.
        for ch in ['X', 'ç'] {
            let first = cache.get(&ctx, ch, &small);
            let again = cache.get(&ctx, ch, &small);
            assert!(
                std::sync::Arc::ptr_eq(&first, &again),
                "{ch} was laid out twice"
            );
        }

        // A different font must not be served the old entries.
        let large = FontId::monospace(20.0);
        cache.prepare(&ctx, &large);
        let big = cache.get(&ctx, 'X', &large);
        cache.prepare(&ctx, &small);
        let small_again = cache.get(&ctx, 'X', &small);
        assert!(
            big.size().x > small_again.size().x,
            "the cache served a stale size: {} vs {}",
            big.size().x,
            small_again.size().x
        );
    }
    #[test]
    fn a_cached_glyph_carries_no_colour_of_its_own() {
        let ctx = egui::Context::default();
        let _ = ctx.run(egui::RawInput::default(), |_| {});
        let mut cache = GlyphCache::default();
        let font = FontId::monospace(14.0);
        cache.prepare(&ctx, &font);
        let galley = cache.get(&ctx, 'X', &font);
        // Every vertex left as PLACEHOLDER is what lets one entry be drawn in
        // whatever colour the cell needs; a real colour baked in here would
        // paint the whole screen one colour.
        let coloured = galley
            .rows
            .iter()
            .flat_map(|row| row.visuals.mesh.vertices.iter())
            .find(|v| v.color != Color32::PLACEHOLDER);
        assert!(coloured.is_none(), "a colour was baked into the cache");
    }
}
