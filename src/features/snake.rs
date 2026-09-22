//! The snake, for the tab that `/snake` opens.
//!
//! An easter egg, and deliberately a self-contained one: it knows nothing of
//! egui, of the grid or of a session, so what draws it is free to draw it any
//! way it likes and the rules can be tested without either. See
//! [`crate::ui::snake_view`] for the board on screen and
//! `App::take_easter_egg` for what opens it.
//!
//! The board is square and always the same number of cells however large the
//! window is - a snake that got shorter relative to its board as the window
//! grew would be a different game in every tab.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Cells along one side of the board.
///
/// Square because the game is: a rectangle would make a turn cost differently
/// depending on which way it was made. 20 leaves cells big enough to see at
/// the size a terminal pane usually is, and a board small enough that filling
/// it is a thing somebody could actually do.
pub const SIDE: usize = 20;

/// How long a step takes at the start, and how much of that each piece of food
/// takes off it. The floor is where it stops getting faster, because below it
/// the snake outruns the eye.
const START_STEP_MS: u64 = 130;
const SPEED_UP_MS: u64 = 3;
const FASTEST_STEP_MS: u64 = 60;

/// How far behind the clock the game may be before it stops trying to catch
/// up.
///
/// A tab that is not on screen is not drawn, so nothing advances it - and
/// coming back to it after a minute elsewhere would otherwise run a minute's
/// worth of steps in one frame and drive the snake straight into a wall. Past
/// this, the missed time is simply dropped.
const MAX_CATCH_UP: Duration = Duration::from_millis(250);

/// A cell of the board: row then column, both counted from the top left.
type Cell = (usize, usize);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    Up,
    Down,
    Left,
    Right,
}

impl Dir {
    fn delta(self) -> (i32, i32) {
        match self {
            Dir::Up => (-1, 0),
            Dir::Down => (1, 0),
            Dir::Left => (0, -1),
            Dir::Right => (0, 1),
        }
    }

    fn opposite(self) -> Dir {
        match self {
            Dir::Up => Dir::Down,
            Dir::Down => Dir::Up,
            Dir::Left => Dir::Right,
            Dir::Right => Dir::Left,
        }
    }
}

/// What the board is doing, which is also what the message across it says.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// Set up and waiting to be started, which is where a new game and a
    /// restarted one both begin.
    Ready,
    Running,
    /// Ran into a wall or into itself.
    Over,
    /// Filled the board: there is nowhere left to put food.
    Won,
}

pub struct Snake {
    /// Every cell the snake is on, head first. A deque because the two ends
    /// are exactly what a step moves: one cell on at the front, one off at the
    /// back.
    body: VecDeque<Cell>,
    dir: Dir,
    /// The turn asked for since the last step, applied when that step comes.
    ///
    /// Held rather than acted on so that two keys pressed inside one step
    /// cannot turn the snake twice: with the direction changed on the spot,
    /// Up-then-Left while heading Right would double back into its own neck.
    pending: Option<Dir>,
    /// Where the food is, or `None` when there is nowhere left to put it -
    /// which is the board being full, and the game being won.
    food: Option<Cell>,
    status: Status,
    score: u32,
    high: u32,
    /// Where the best score is kept between runs. `None` keeps it in memory
    /// only, which is what a test uses: the config directory is shared with
    /// the installed app.
    file: Option<PathBuf>,
    rng: Rng,
    /// When the last step was taken, in game time rather than wall time - see
    /// [`Snake::advance`].
    stepped_at: Instant,
}

impl Snake {
    /// A new game, ready to start, with the best score read back from `file`.
    pub fn new(file: Option<PathBuf>) -> Self {
        let high = file.as_deref().map(read_high).unwrap_or(0);
        let mut game = Snake {
            body: VecDeque::new(),
            dir: Dir::Right,
            pending: None,
            food: None,
            status: Status::Ready,
            score: 0,
            high,
            file,
            rng: Rng::seeded(),
            stepped_at: Instant::now(),
        };
        game.restart();
        game
    }

    /// Back to a fresh board, keeping the best score.
    pub fn restart(&mut self) {
        let middle = SIDE / 2;
        // Three cells, laid out to the left of the head, so the opening
        // heading has somewhere to have come from.
        self.body = (0..3).map(|i| (middle, middle - i)).collect();
        self.dir = Dir::Right;
        self.pending = None;
        self.score = 0;
        self.status = Status::Ready;
        self.food = self.place_food();
        self.stepped_at = Instant::now();
    }

    pub fn status(&self) -> Status {
        self.status
    }

    pub fn score(&self) -> u32 {
        self.score
    }

    pub fn high_score(&self) -> u32 {
        self.high
    }

    pub fn food(&self) -> Option<Cell> {
        self.food
    }

    /// The snake itself, head first.
    pub fn body(&self) -> impl Iterator<Item = Cell> + '_ {
        self.body.iter().copied()
    }

    /// How long one step lasts at the length the snake is now. What the view
    /// asks for its next frame after, so an idle board costs nothing.
    pub fn interval(&self) -> Duration {
        let ms = START_STEP_MS
            .saturating_sub(u64::from(self.score) * SPEED_UP_MS)
            .max(FASTEST_STEP_MS);
        Duration::from_millis(ms)
    }

    /// Starts a game that is waiting, or starts the next one when the last is
    /// finished. What Space does.
    pub fn start(&mut self) {
        match self.status {
            Status::Ready => {
                self.status = Status::Running;
                self.stepped_at = Instant::now();
            }
            Status::Over | Status::Won => {
                self.restart();
                self.status = Status::Running;
                self.stepped_at = Instant::now();
            }
            Status::Running => {}
        }
    }

    /// Asks for a turn, which is refused if it would double the snake back
    /// along itself.
    ///
    /// Measured against the heading rather than against a turn already
    /// waiting, and that is the exact rule rather than an approximation of it:
    /// the head has not moved yet, so the neck is still the cell opposite
    /// `dir` whatever was asked for a moment ago. It is also what stops two
    /// keys pressed inside one step from adding up to a reversal - Up and then
    /// Left while heading Right is a Left that is still backwards, and is
    /// refused.
    pub fn turn(&mut self, dir: Dir) {
        if self.status == Status::Ready {
            self.start();
        }
        if self.status != Status::Running {
            return;
        }
        if dir == self.dir.opposite() {
            return;
        }
        self.pending = Some(dir);
    }

    /// Takes whatever steps `now` has fallen due, and none at all when the
    /// game is not running.
    pub fn advance(&mut self, now: Instant) {
        if self.status != Status::Running {
            self.stepped_at = now;
            return;
        }
        if now.duration_since(self.stepped_at) > MAX_CATCH_UP {
            self.stepped_at = now;
            return;
        }
        while self.status == Status::Running
            && now.duration_since(self.stepped_at) >= self.interval()
        {
            self.stepped_at += self.interval();
            self.step();
        }
    }

    /// One move of the snake: the rules of the game, and all of them.
    pub fn step(&mut self) {
        if self.status != Status::Running {
            return;
        }
        if let Some(dir) = self.pending.take() {
            self.dir = dir;
        }
        let Some(&(row, col)) = self.body.front() else {
            return;
        };
        let (down, right) = self.dir.delta();
        let (row, col) = (row as i32 + down, col as i32 + right);
        // The walls are walls. Wrapping around them would be a gentler game
        // and a different one.
        if row < 0 || col < 0 || row >= SIDE as i32 || col >= SIDE as i32 {
            self.finish(Status::Over);
            return;
        }
        let head = (row as usize, col as usize);

        // The tail comes off before the collision is looked for, so following
        // your own tail - which moves out of the way in the same step - is
        // allowed. Not when the head landed on food: that is the step the
        // snake grows in, and the tail stays where it is.
        let eaten = self.food == Some(head);
        if !eaten {
            self.body.pop_back();
        }
        if self.body.contains(&head) {
            self.finish(Status::Over);
            return;
        }
        self.body.push_front(head);

        if eaten {
            self.score += 1;
            self.high = self.high.max(self.score);
            self.food = self.place_food();
            // Nowhere left to put it: every cell of the board is snake, which
            // is the game being finished rather than lost.
            if self.food.is_none() {
                self.finish(Status::Won);
            }
        }
    }

    /// Ends the game and writes the best score down if this one beat it.
    fn finish(&mut self, status: Status) {
        self.status = status;
        self.high = self.high.max(self.score);
        if let Some(path) = self.file.as_deref() {
            let _ = std::fs::write(path, self.high.to_string());
        }
    }

    /// A free cell, picked evenly among the free ones, or `None` when there
    /// are none.
    ///
    /// Counted and then walked rather than guessed at until one lands
    /// somewhere free: with the board nearly full, guessing takes unboundedly
    /// long at exactly the moment the game is at its most tense.
    fn place_food(&mut self) -> Option<Cell> {
        let free = SIDE * SIDE - self.body.len();
        if free == 0 {
            return None;
        }
        let mut wanted = self.rng.below(free);
        for row in 0..SIDE {
            for col in 0..SIDE {
                if self.body.contains(&(row, col)) {
                    continue;
                }
                if wanted == 0 {
                    return Some((row, col));
                }
                wanted -= 1;
            }
        }
        None
    }
}

/// The best score in `path`, or none at all if there is nothing readable
/// there. A file that has been edited by hand is not worth complaining about.
fn read_high(path: &Path) -> u32 {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| text.trim().parse().ok())
        .unwrap_or(0)
}

/// Where the food goes next.
///
/// An xorshift rather than a dependency: the only thing asked of it is that
/// the food not be predictable within a game, and sixty-four bits of state
/// that fit in a register answer that completely.
struct Rng(u64);

impl Rng {
    fn seeded() -> Rng {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|since| since.as_nanos() as u64)
            .unwrap_or(0x2545_F491_4F6C_DD1D);
        // Never zero: xorshift stays at zero forever once it gets there.
        Rng(nanos | 1)
    }

    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    /// A number under `bound`, which must not be zero.
    fn below(&mut self, bound: usize) -> usize {
        (self.next() % bound as u64) as usize
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A game in memory, started, with the snake where `restart` puts it.
    fn started() -> Snake {
        let mut game = Snake::new(None);
        game.start();
        game
    }

    /// Walks the snake into the right-hand wall and reports where it ended up.
    fn run_into_the_wall() -> Snake {
        let mut game = started();
        // Food is in the way of nothing in particular, so it is taken off the
        // board: this test is about the wall.
        game.food = None;
        for _ in 0..SIDE {
            game.step();
        }
        game
    }

    #[test]
    fn the_snake_dies_against_a_wall_rather_than_wrapping_around_it() {
        let game = run_into_the_wall();
        assert_eq!(game.status(), Status::Over);
        assert!(
            game.body().all(|(_, col)| col < SIDE),
            "nothing should be left outside the board"
        );
    }

    #[test]
    fn the_snake_dies_on_its_own_body() {
        let mut game = started();
        game.food = None;
        // Long enough to bite, and curled so that one turn puts the head in
        // the middle of it rather than on the tail.
        game.body = VecDeque::from(vec![(5, 5), (5, 4), (5, 3), (6, 3), (6, 4), (6, 5), (6, 6)]);
        game.dir = Dir::Right;
        game.turn(Dir::Down);
        game.step();
        assert_eq!(game.status(), Status::Over);
    }

    #[test]
    fn eating_lengthens_the_snake_and_scores_one() {
        let mut game = started();
        let (row, col) = game.body().next().expect("a head");
        game.food = Some((row, col + 1));
        let length = game.body().count();
        game.step();
        assert_eq!(game.score(), 1);
        assert_eq!(game.body().count(), length + 1);
        assert_eq!(game.status(), Status::Running);
    }

    /// Following your own tail is legal: it has moved on by the time the head
    /// gets there.
    #[test]
    fn the_snake_may_move_into_the_cell_its_tail_is_leaving() {
        let mut game = started();
        game.food = None;
        // Curled so the head is right above its own tail, and then turned
        // onto it.
        game.body = VecDeque::from(vec![(5, 5), (5, 4), (6, 4), (6, 5)]);
        game.dir = Dir::Right;
        game.turn(Dir::Down);
        game.step();
        assert_eq!(game.status(), Status::Running);
        assert_eq!(game.body().next(), Some((6, 5)));
    }

    #[test]
    fn a_turn_back_along_the_snake_is_refused() {
        let mut game = started();
        game.turn(Dir::Left);
        game.step();
        assert_eq!(game.dir, Dir::Right, "heading right, Left is backwards");
    }

    /// Two keys inside one step must not add up to a reversal: Up and then
    /// Left, while heading Right, would otherwise put the head in its own
    /// neck.
    #[test]
    fn two_turns_in_one_step_cannot_add_up_to_a_reversal() {
        let mut game = started();
        game.turn(Dir::Up);
        game.turn(Dir::Left);
        game.step();
        assert_eq!(game.dir, Dir::Up);
        assert_eq!(game.status(), Status::Running);
    }

    #[test]
    fn food_never_lands_on_the_snake() {
        let mut game = started();
        for _ in 0..200 {
            let food = game.place_food().expect("room for food");
            assert!(!game.body().any(|cell| cell == food));
        }
    }

    /// The end of the game as the rules have it: the board is full, so there
    /// is nowhere to put the next piece of food.
    #[test]
    fn filling_the_board_wins_rather_than_ending_the_game() {
        let mut game = started();
        // Every cell but the one the head is about to move into, which the
        // sweep leaves at the front of the queue: the head is at the top left,
        // heading right into the one free cell, with the last piece of food on
        // it.
        game.body = (0..SIDE)
            .flat_map(|row| (0..SIDE).map(move |col| (row, col)))
            .filter(|&cell| cell != (0, 1))
            .collect();
        game.dir = Dir::Right;
        game.food = Some((0, 1));
        game.step();
        assert_eq!(game.status(), Status::Won);
        assert_eq!(game.body().count(), SIDE * SIDE);
    }

    #[test]
    fn the_best_score_survives_the_game_it_was_set_in() {
        let mut game = started();
        game.score = 7;
        game.finish(Status::Over);
        assert_eq!(game.high_score(), 7);
        game.start();
        assert_eq!(game.score(), 0);
        assert_eq!(game.high_score(), 7, "a new game keeps the record");
        assert_eq!(game.status(), Status::Running);
    }

    /// The snake gets faster as it eats, and stops getting faster before it
    /// gets faster than the eye.
    #[test]
    fn the_step_shortens_with_the_score_and_then_stops() {
        let mut game = started();
        let opening = game.interval();
        game.score = 5;
        assert!(game.interval() < opening);
        game.score = 10_000;
        assert_eq!(game.interval(), Duration::from_millis(FASTEST_STEP_MS));
    }

    /// A tab left in the background is not drawn, so nothing advances it.
    /// Coming back to it must not run the missed minute all at once.
    #[test]
    fn time_spent_in_another_tab_is_dropped_rather_than_caught_up_on() {
        let mut game = started();
        let head = game.body().next().expect("a head");
        game.advance(Instant::now() + Duration::from_secs(30));
        assert_eq!(game.body().next(), Some(head), "no step should have run");
        assert_eq!(game.status(), Status::Running);
    }
}
