//! Frame-time statistics for the debug overlay (ADR-0003 Decision §3:
//! median and 95th percentile over 10 s of panning and zooming).

use std::collections::VecDeque;

/// About 10 s at 60 frames per second.
pub const WINDOW: usize = 600;

/// The last [`WINDOW`] samples, in seconds.
#[derive(Debug, Default)]
pub struct FrameTimes(VecDeque<f32>);

impl FrameTimes {
    /// Adds a sample, dropping the oldest once the window is full.
    pub fn push(&mut self, seconds: f32) {
        if self.0.len() == WINDOW {
            self.0.pop_front();
        }
        self.0.push_back(seconds);
    }

    /// Adds the sample, if any.
    pub fn extend(&mut self, seconds: Option<f32>) {
        if let Some(s) = seconds {
            self.push(s);
        }
    }

    /// Number of samples held.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// `(median, p95)` by nearest rank, or `None` without samples.
    pub fn median_p95(&self) -> Option<(f32, f32)> {
        let mut sorted: Vec<f32> = self.0.iter().copied().collect();
        sorted.sort_by(f32::total_cmp);
        let rank = |p: f32| {
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_sign_loss,
                clippy::cast_precision_loss,
                reason = "a rank within the window"
            )]
            let r = (p * sorted.len() as f32).ceil() as usize;
            sorted[r.max(1) - 1]
        };
        (!sorted.is_empty()).then(|| (rank(0.5), rank(0.95)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn median_and_p95_by_nearest_rank() {
        let mut t = FrameTimes::default();
        assert_eq!(t.median_p95(), None);
        for ms in (1..=100).rev() {
            t.push(f32::from(u8::try_from(ms).unwrap()));
        }
        assert_eq!(t.median_p95(), Some((50.0, 95.0)));
        t = FrameTimes::default();
        t.push(7.0);
        assert_eq!(t.median_p95(), Some((7.0, 7.0)));
    }

    #[test]
    fn only_the_last_window_counts() {
        let mut t = FrameTimes::default();
        for _ in 0..WINDOW {
            t.push(1.0);
        }
        for _ in 0..WINDOW {
            t.push(2.0);
        }
        assert_eq!(t.len(), WINDOW);
        assert_eq!(t.median_p95(), Some((2.0, 2.0)));
    }
}
