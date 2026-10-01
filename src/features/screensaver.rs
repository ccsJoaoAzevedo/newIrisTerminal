//! The screen saver: what it is set to, and what moves on it.
//!
//! Only the motion lives here - where the logo is, which column of the rain is
//! how far down - so it can be stepped and checked without a window. Drawing
//! it is [`crate::ui::screensaver_view`]'s job, and deciding when it starts is
//! the frame loop's.

use serde::{Deserialize, Serialize};

/// Which screen saver runs. `None` means none ever does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    #[default]
    None,
    /// The Windows XP logo, fading in somewhere, then out, then elsewhere.
    XpLogo,
    /// Green characters falling down the window.
    Matrix,
    /// The bouncing DVD logo - which, as everyone who ever watched one knows,
    /// never quite hits the corner.
    Dvd,
}

impl Kind {
    pub const ALL: [Kind; 4] = [Kind::None, Kind::XpLogo, Kind::Matrix, Kind::Dvd];

    /// English, for `tr`.
    pub fn label(self) -> &'static str {
        match self {
            Kind::None => "(None)",
            Kind::XpLogo => "Windows XP",
            Kind::Matrix => "Matrix",
            Kind::Dvd => "DVD idle",
        }
    }
}

/// The screen saver settings, as they are saved.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub kind: Kind,
    /// Minutes without a key or a mouse movement before it starts.
    pub wait_minutes: u32,
    /// Multiplies every speed. 1 is the pace each saver was tuned at.
    pub speed: f32,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            kind: Kind::None,
            wait_minutes: 10,
            speed: 1.0,
        }
    }
}

impl Config {
    pub fn wait(&self) -> std::time::Duration {
        std::time::Duration::from_secs(u64::from(self.wait_minutes.max(1)) * 60)
    }
}

/// xorshift: the savers need something that looks random, not something that
/// is, and a dependency for that would be out of proportion.
#[derive(Clone, Debug)]
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed | 1)
    }

    pub fn next_u32(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        (x >> 32) as u32
    }

    /// In `[0, 1)`.
    pub fn unit(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 / (1u32 << 24) as f32
    }

    pub fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.unit()
    }
}

/// How long one appearance of the XP logo lasts, in seconds, and how much of
/// either end of it is spent fading.
const XP_CYCLE: f32 = 7.0;
const XP_FADE: f32 = 1.2;

/// The XP logo: it does not travel, it turns up. Each appearance is at a new
/// place, faded in, held, and faded out again, which is what the real one did.
#[derive(Clone, Debug)]
pub struct XpLogo {
    /// Top-left, as a fraction of the room the logo has to move in.
    pub at: (f32, f32),
    clock: f32,
}

impl XpLogo {
    fn new(rng: &mut Rng) -> Self {
        XpLogo {
            at: (rng.unit(), rng.unit()),
            clock: 0.0,
        }
    }

    fn step(&mut self, dt: f32, rng: &mut Rng) {
        self.clock += dt;
        if self.clock >= XP_CYCLE {
            self.clock %= XP_CYCLE;
            self.at = (rng.unit(), rng.unit());
        }
    }

    /// How visible the logo is right now, 0 to 1.
    pub fn alpha(&self) -> f32 {
        let t = self.clock;
        if t < XP_FADE {
            t / XP_FADE
        } else if t > XP_CYCLE - XP_FADE {
            (XP_CYCLE - t) / XP_FADE
        } else {
            1.0
        }
        .clamp(0.0, 1.0)
    }
}

/// One falling column of the rain.
#[derive(Clone, Debug)]
pub struct Column {
    /// Where the bright head is, in rows; negative while it is still above the
    /// window.
    pub head: f32,
    /// Rows per second.
    pub speed: f32,
    /// How many rows of fading trail follow the head.
    pub trail: usize,
    /// One glyph per row, swapped out now and then as the head passes.
    pub glyphs: Vec<char>,
}

/// What the rain is drawn in. ASCII and a few symbols rather than katakana:
/// the bundled fonts have no katakana, and a column of missing-glyph boxes is
/// not the effect.
const RAIN: &[char] = &[
    '0', '1', '2', '3', '4', '5', '6', '7', '8', '9', 'A', 'B', 'C', 'D', 'E', 'F', 'Z', 'X', 'K',
    'M', 'R', 'T', '$', '^', '*', '+', '-', '=', '<', '>', ':', '|', '"', '#', '%', '&',
];

#[derive(Clone, Debug)]
pub struct Matrix {
    pub columns: Vec<Column>,
    pub rows: usize,
}

impl Matrix {
    fn new() -> Self {
        Matrix {
            columns: Vec::new(),
            rows: 0,
        }
    }

    fn column(rows: usize, rng: &mut Rng, start_above: bool) -> Column {
        let trail = 6 + (rng.next_u32() % 20) as usize;
        Column {
            head: if start_above {
                -rng.range(0.0, rows as f32)
            } else {
                rng.range(0.0, rows as f32)
            },
            speed: rng.range(6.0, 22.0),
            trail,
            glyphs: (0..rows.max(1)).map(|_| pick(rng)).collect(),
        }
    }

    /// Fits the rain to a grid of `cols` by `rows`. A resize starts the
    /// columns over: stretching them would have every trail jump at once.
    fn fit(&mut self, cols: usize, rows: usize, rng: &mut Rng) {
        if self.columns.len() == cols && self.rows == rows {
            return;
        }
        self.rows = rows;
        self.columns = (0..cols).map(|_| Self::column(rows, rng, false)).collect();
    }

    fn step(&mut self, dt: f32, rng: &mut Rng) {
        let rows = self.rows;
        for column in &mut self.columns {
            let before = column.head.floor();
            column.head += column.speed * dt;
            // A new glyph under the head whenever it enters a row, and the odd
            // one in the trail, which is what makes the trail shimmer.
            if column.head.floor() != before && column.head >= 0.0 {
                let row = column.head as usize;
                if let Some(slot) = column.glyphs.get_mut(row) {
                    *slot = pick(rng);
                }
            }
            if rng.unit() < dt * 2.0 && !column.glyphs.is_empty() {
                let row = rng.next_u32() as usize % column.glyphs.len();
                column.glyphs[row] = pick(rng);
            }
            if column.head - column.trail as f32 > rows as f32 {
                *column = Self::column(rows, rng, true);
            }
        }
    }
}

fn pick(rng: &mut Rng) -> char {
    RAIN[rng.next_u32() as usize % RAIN.len()]
}

/// Pixels per second the DVD logo travels at, along each axis.
const DVD_SPEED: f32 = 140.0;

/// How close to a corner, in seconds of travel, counts as about to hit it.
///
/// Both walls arriving within this of each other is a corner, and the logo is
/// moved somewhere else before it gets there. Large enough that a frame at a
/// low rate cannot step over it.
const DVD_NEAR_MISS: f32 = 0.25;

/// The DVD logo bouncing round the window.
///
/// The joke is the one every office watched for: it never quite reaches the
/// corner. Just as it is about to, it is somewhere else.
#[derive(Clone, Debug)]
pub struct Dvd {
    /// Top-left, in pixels within the room.
    pub pos: (f32, f32),
    /// Pixels per second.
    pub vel: (f32, f32),
    /// Which colour of the cycle it is in; moves on at every bounce.
    pub hue: usize,
    /// How many times it has been snatched away from a corner.
    pub near_misses: u32,
    /// Not yet placed in a real window: the room is only known at the first
    /// step, and starting it in a corner of nothing would start it in a corner.
    fresh: bool,
}

impl Dvd {
    fn new(rng: &mut Rng) -> Self {
        Dvd {
            pos: (0.0, 0.0),
            vel: (0.0, 0.0),
            hue: 0,
            near_misses: 0,
            fresh: true,
        }
        .placed(rng, (1.0, 1.0), 1.0)
    }

    /// Somewhere new in `room` (the window less the logo), heading off on a
    /// diagonal, and never on a course that is already about to hit a corner.
    fn placed(mut self, rng: &mut Rng, room: (f32, f32), speed: f32) -> Self {
        for _ in 0..16 {
            self.pos = (rng.unit() * room.0, rng.unit() * room.1);
            let sx = if rng.unit() < 0.5 { -1.0 } else { 1.0 };
            let sy = if rng.unit() < 0.5 { -1.0 } else { 1.0 };
            self.vel = (sx * DVD_SPEED * speed, sy * DVD_SPEED * speed);
            if !self.heading_for_corner(room) {
                break;
            }
        }
        self
    }

    /// Seconds until it reaches the wall it is moving towards, on one axis.
    fn until_wall(pos: f32, vel: f32, room: f32) -> f32 {
        if vel > 0.0 {
            (room - pos) / vel
        } else if vel < 0.0 {
            pos / -vel
        } else {
            f32::INFINITY
        }
    }

    /// Whether both walls are coming up together, which is a corner.
    fn heading_for_corner(&self, room: (f32, f32)) -> bool {
        let tx = Self::until_wall(self.pos.0, self.vel.0, room.0);
        let ty = Self::until_wall(self.pos.1, self.vel.1, room.1);
        tx.max(ty) < DVD_NEAR_MISS * 2.0 && (tx - ty).abs() < DVD_NEAR_MISS
    }

    fn step(&mut self, dt: f32, room: (f32, f32), speed: f32, rng: &mut Rng) {
        let room = (room.0.max(1.0), room.1.max(1.0));
        if self.fresh {
            *self = self.clone().placed(rng, room, speed);
            self.fresh = false;
        }
        // A window that shrank under it leaves it outside; it is put back on
        // the floor rather than left bouncing off a wall it is beyond.
        self.pos.0 = self.pos.0.clamp(0.0, room.0);
        self.pos.1 = self.pos.1.clamp(0.0, room.1);
        // The speed setting can change while it runs.
        self.vel.0 = self.vel.0.signum() * DVD_SPEED * speed;
        self.vel.1 = self.vel.1.signum() * DVD_SPEED * speed;

        if self.heading_for_corner(room) {
            let (hue, misses) = (self.hue + 1, self.near_misses + 1);
            *self = self.clone().placed(rng, room, speed);
            self.hue = hue;
            self.near_misses = misses;
            return;
        }

        self.pos.0 += self.vel.0 * dt;
        self.pos.1 += self.vel.1 * dt;
        let mut bounced = false;
        if self.pos.0 <= 0.0 || self.pos.0 >= room.0 {
            self.vel.0 = -self.vel.0;
            self.pos.0 = self.pos.0.clamp(0.0, room.0);
            bounced = true;
        }
        if self.pos.1 <= 0.0 || self.pos.1 >= room.1 {
            self.vel.1 = -self.vel.1;
            self.pos.1 = self.pos.1.clamp(0.0, room.1);
            bounced = true;
        }
        if bounced {
            self.hue += 1;
        }
    }
}

/// What is on screen, of whichever kind.
#[derive(Clone, Debug)]
pub enum Scene {
    XpLogo(XpLogo),
    Matrix(Matrix),
    Dvd(Dvd),
}

/// A running screen saver: a scene and the randomness that moves it.
#[derive(Clone, Debug)]
pub struct Saver {
    pub kind: Kind,
    pub scene: Option<Scene>,
    rng: Rng,
}

impl Saver {
    pub fn new(kind: Kind, seed: u64) -> Self {
        let mut rng = Rng::new(seed);
        let scene = match kind {
            Kind::None => None,
            Kind::XpLogo => Some(Scene::XpLogo(XpLogo::new(&mut rng))),
            Kind::Matrix => Some(Scene::Matrix(Matrix::new())),
            Kind::Dvd => Some(Scene::Dvd(Dvd::new(&mut rng))),
        };
        Saver { kind, scene, rng }
    }

    /// Moves everything on by `dt` seconds.
    ///
    /// `room` is the space a logo can travel in - the window less the logo -
    /// and `grid` the rain's columns and rows; each scene reads the one it
    /// needs. `dt` is capped, so a frame that arrives late (the window was
    /// dragged, the machine was busy) is not a jump across the screen.
    pub fn step(&mut self, dt: f32, speed: f32, room: (f32, f32), grid: (usize, usize)) {
        let dt = dt.clamp(0.0, 0.1) * speed.clamp(0.1, 5.0);
        let speed = speed.clamp(0.1, 5.0);
        let rng = &mut self.rng;
        match self.scene.as_mut() {
            None => {}
            Some(Scene::XpLogo(logo)) => logo.step(dt, rng),
            Some(Scene::Matrix(rain)) => {
                rain.fit(grid.0, grid.1, rng);
                rain.step(dt, rng);
            }
            // The logo's own speed already carries the setting.
            Some(Scene::Dvd(dvd)) => dvd.step(dt / speed, room, speed, rng),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The whole of the joke, and so the thing worth pinning: run it for an
    /// hour of frames in a few window shapes and it never touches two walls
    /// at once.
    #[test]
    fn the_dvd_logo_never_reaches_a_corner() {
        for (seed, room) in [
            (1, (640.0, 360.0)),
            (7, (300.0, 300.0)),
            (42, (1200.0, 500.0)),
        ] {
            let mut saver = Saver::new(Kind::Dvd, seed);
            for _ in 0..(60 * 60 * 30) {
                saver.step(1.0 / 30.0, 1.0, room, (0, 0));
                let Some(Scene::Dvd(dvd)) = &saver.scene else {
                    unreachable!()
                };
                let edge_x = dvd.pos.0 <= 0.5 || dvd.pos.0 >= room.0 - 0.5;
                let edge_y = dvd.pos.1 <= 0.5 || dvd.pos.1 >= room.1 - 0.5;
                assert!(!(edge_x && edge_y), "hit a corner at {:?}", dvd.pos);
            }
        }
    }

    /// And it has to have been about to, now and then, or the guard is never
    /// what kept it out.
    #[test]
    fn the_dvd_logo_is_snatched_away_from_corners_it_was_heading_for() {
        let mut saver = Saver::new(Kind::Dvd, 3);
        for _ in 0..(60 * 60 * 30) {
            saver.step(1.0 / 30.0, 1.0, (320.0, 180.0), (0, 0));
        }
        let Some(Scene::Dvd(dvd)) = &saver.scene else {
            unreachable!()
        };
        assert!(dvd.near_misses > 0);
    }

    #[test]
    fn the_dvd_logo_stays_inside_a_window_that_shrinks_under_it() {
        let mut saver = Saver::new(Kind::Dvd, 9);
        for _ in 0..300 {
            saver.step(1.0 / 30.0, 1.0, (800.0, 600.0), (0, 0));
        }
        for _ in 0..300 {
            saver.step(1.0 / 30.0, 1.0, (100.0, 80.0), (0, 0));
            let Some(Scene::Dvd(dvd)) = &saver.scene else {
                unreachable!()
            };
            assert!((0.0..=100.0).contains(&dvd.pos.0));
            assert!((0.0..=80.0).contains(&dvd.pos.1));
        }
    }

    #[test]
    fn the_xp_logo_fades_in_and_out_and_turns_up_somewhere_else() {
        let mut saver = Saver::new(Kind::XpLogo, 5);
        let at = |s: &Saver| match &s.scene {
            Some(Scene::XpLogo(l)) => (l.at, l.alpha()),
            _ => unreachable!(),
        };
        let (first, alpha) = at(&saver);
        assert_eq!(alpha, 0.0, "starts invisible");
        for _ in 0..(3 * 30) {
            saver.step(1.0 / 30.0, 1.0, (0.0, 0.0), (0, 0));
        }
        assert_eq!(at(&saver).1, 1.0, "fully shown mid-cycle");
        for _ in 0..(5 * 30) {
            saver.step(1.0 / 30.0, 1.0, (0.0, 0.0), (0, 0));
        }
        assert_ne!(at(&saver).0, first, "moved for its next appearance");
    }

    #[test]
    fn the_rain_fills_the_grid_and_every_column_keeps_falling() {
        let mut saver = Saver::new(Kind::Matrix, 11);
        saver.step(1.0 / 30.0, 1.0, (0.0, 0.0), (40, 20));
        let heads = |s: &Saver| match &s.scene {
            Some(Scene::Matrix(m)) => m.columns.iter().map(|c| c.head).collect::<Vec<_>>(),
            _ => unreachable!(),
        };
        let before = heads(&saver);
        assert_eq!(before.len(), 40);
        saver.step(1.0 / 30.0, 1.0, (0.0, 0.0), (40, 20));
        for (a, b) in before.iter().zip(heads(&saver)) {
            assert!(b > *a || b < 0.0, "a column stood still");
        }
    }

    #[test]
    fn a_settings_file_without_a_screen_saver_has_none() {
        let config: Config = toml::from_str("").unwrap();
        assert_eq!(config.kind, Kind::None);
        let back: Config = toml::from_str(
            &toml::to_string(&Config {
                kind: Kind::Dvd,
                wait_minutes: 3,
                speed: 1.5,
            })
            .unwrap(),
        )
        .unwrap();
        assert_eq!(back.kind, Kind::Dvd);
        assert_eq!(back.wait_minutes, 3);
    }
}
