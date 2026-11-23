//! Play-order logic: which track comes next, with shuffle and repeat.
//!
//! Pure and platform-agnostic. The queue stores indices into a track list
//! owned elsewhere, so it never touches paths or audio.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Repeat {
    #[default]
    Off,
    /// Wrap around to the start when the end is reached.
    All,
    /// Replay the current track when it ends (manual next/prev still move).
    One,
}

impl Repeat {
    pub fn cycle(self) -> Repeat {
        match self {
            Repeat::Off => Repeat::All,
            Repeat::All => Repeat::One,
            Repeat::One => Repeat::Off,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Queue {
    /// Play order: a permutation of `0..len`.
    order: Vec<usize>,
    /// Position in `order` of the current track.
    pos: usize,
    shuffle: bool,
    repeat: Repeat,
    rng: u64,
}

impl Queue {
    /// A queue over `len` tracks starting at track `start`. Returns `None`
    /// for an empty list.
    pub fn new(len: usize, start: usize, shuffle: bool, seed: u64) -> Option<Queue> {
        if len == 0 {
            return None;
        }
        let mut q = Queue {
            order: (0..len).collect(),
            pos: start.min(len - 1),
            shuffle: false,
            repeat: Repeat::Off,
            rng: seed | 1,
        };
        q.set_shuffle(shuffle);
        Some(q)
    }

    pub fn len(&self) -> usize {
        self.order.len()
    }

    pub fn is_empty(&self) -> bool {
        self.order.is_empty()
    }

    /// Track index currently selected.
    pub fn current(&self) -> usize {
        self.order[self.pos]
    }

    /// Track indices in play order, and the position of the current one.
    pub fn order(&self) -> (&[usize], usize) {
        (&self.order, self.pos)
    }

    pub fn shuffle(&self) -> bool {
        self.shuffle
    }

    pub fn repeat(&self) -> Repeat {
        self.repeat
    }

    pub fn set_repeat(&mut self, repeat: Repeat) {
        self.repeat = repeat;
    }

    /// Turning shuffle on keeps the current track playing and shuffles the
    /// rest after it; turning it off returns to list order at the same track.
    pub fn set_shuffle(&mut self, on: bool) {
        let current = self.current();
        self.shuffle = on;
        self.order = (0..self.order.len()).collect();
        if on {
            self.order.swap(0, current);
            self.shuffle_from(1);
            self.pos = 0;
        } else {
            self.pos = current;
        }
    }

    /// Selects a specific track (e.g. chosen in the library view).
    pub fn jump_to(&mut self, track: usize) {
        if let Some(p) = self.order.iter().position(|&t| t == track) {
            self.pos = p;
        }
    }

    /// The track that should follow when the current one finishes on its
    /// own, without changing state. Used to pre-open the next file for
    /// gapless playback.
    pub fn peek_auto(&self) -> Option<usize> {
        match self.repeat {
            Repeat::One => Some(self.current()),
            _ => self.peek_next(),
        }
    }

    fn peek_next(&self) -> Option<usize> {
        if self.pos + 1 < self.order.len() {
            Some(self.order[self.pos + 1])
        } else if self.repeat != Repeat::Off {
            // Wrapping; with shuffle the order is reshuffled on wrap, but the
            // first track of the new order is decided now so that peek and
            // advance agree.
            Some(self.order[0])
        } else {
            None
        }
    }

    /// The current track ended naturally. Returns the next track, or `None`
    /// when playback should stop.
    pub fn advance_auto(&mut self) -> Option<usize> {
        match self.repeat {
            Repeat::One => Some(self.current()),
            _ => self.step_forward(),
        }
    }

    /// The user pressed next. Repeat-one does not trap the user on a track.
    pub fn skip_forward(&mut self) -> Option<usize> {
        self.step_forward()
    }

    /// The user pressed previous. At the start, wraps only with repeat on;
    /// otherwise stays on the first track.
    pub fn skip_back(&mut self) -> usize {
        if self.pos > 0 {
            self.pos -= 1;
        } else if self.repeat != Repeat::Off {
            self.pos = self.order.len() - 1;
        }
        self.current()
    }

    fn step_forward(&mut self) -> Option<usize> {
        if self.pos + 1 < self.order.len() {
            self.pos += 1;
            Some(self.current())
        } else if self.repeat != Repeat::Off {
            if self.shuffle && self.order.len() > 2 {
                // New shuffle for the next lap, keeping the track promised by
                // `peek_next` (order[0]) first.
                self.shuffle_from(1);
            }
            self.pos = 0;
            Some(self.current())
        } else {
            None
        }
    }

    /// Fisher-Yates shuffle of `order[from..]` with xorshift64*.
    fn shuffle_from(&mut self, from: usize) {
        for i in (from + 1..self.order.len()).rev() {
            let j = from + (self.next_rand() % (i - from + 1) as u64) as usize;
            self.order.swap(i, j);
        }
    }

    fn next_rand(&mut self) -> u64 {
        self.rng ^= self.rng >> 12;
        self.rng ^= self.rng << 25;
        self.rng ^= self.rng >> 27;
        self.rng.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_is_none() {
        assert!(Queue::new(0, 0, false, 1).is_none());
    }

    #[test]
    fn linear_play_stops_at_end() {
        let mut q = Queue::new(3, 0, false, 1).unwrap();
        assert_eq!(q.current(), 0);
        assert_eq!(q.peek_auto(), Some(1));
        assert_eq!(q.advance_auto(), Some(1));
        assert_eq!(q.advance_auto(), Some(2));
        assert_eq!(q.peek_auto(), None);
        assert_eq!(q.advance_auto(), None);
        assert_eq!(q.current(), 2);
    }

    #[test]
    fn repeat_all_wraps_and_prev_wraps() {
        let mut q = Queue::new(3, 2, false, 1).unwrap();
        q.set_repeat(Repeat::All);
        assert_eq!(q.peek_auto(), Some(0));
        assert_eq!(q.advance_auto(), Some(0));
        assert_eq!(q.skip_back(), 2);
    }

    #[test]
    fn prev_without_repeat_stays_on_first() {
        let mut q = Queue::new(3, 0, false, 1).unwrap();
        assert_eq!(q.skip_back(), 0);
    }

    #[test]
    fn repeat_one_replays_on_auto_but_next_moves() {
        let mut q = Queue::new(3, 1, false, 1).unwrap();
        q.set_repeat(Repeat::One);
        assert_eq!(q.peek_auto(), Some(1));
        assert_eq!(q.advance_auto(), Some(1));
        assert_eq!(q.skip_forward(), Some(2));
    }

    #[test]
    fn shuffle_keeps_current_and_is_a_permutation() {
        let mut q = Queue::new(50, 7, false, 42).unwrap();
        q.set_shuffle(true);
        assert_eq!(q.current(), 7);
        let (order, pos) = q.order();
        assert_eq!(pos, 0);
        let mut sorted = order.to_vec();
        sorted.sort_unstable();
        assert_eq!(sorted, (0..50).collect::<Vec<_>>());
        assert_ne!(order, (0..50).collect::<Vec<_>>().as_slice());

        q.skip_forward();
        let playing = q.current();
        q.set_shuffle(false);
        assert_eq!(q.current(), playing);
    }

    #[test]
    fn shuffle_wrap_honours_peek() {
        let mut q = Queue::new(10, 0, true, 9).unwrap();
        q.set_repeat(Repeat::All);
        for _ in 0..9 {
            q.skip_forward();
        }
        let promised = q.peek_auto().unwrap();
        assert_eq!(q.advance_auto(), Some(promised));
    }

    #[test]
    fn jump_to_selects_track() {
        let mut q = Queue::new(5, 0, true, 3).unwrap();
        q.jump_to(4);
        assert_eq!(q.current(), 4);
    }
}
