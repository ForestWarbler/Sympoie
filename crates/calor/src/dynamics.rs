//! Envelope followers, and what each band's Dynamics control makes of its envelope.
//!
//! Dynamics turned right compresses the band above −24 dBFS, up to 8:1 with a slow enough
//! release to pump; turned left it expands the band below −36 dBFS, up to 1:4, which
//! tightens it towards a gate. The envelope is the band's own input, linked across the
//! channels, so the image stays put.

use crate::params::amplitude;

/// Where compression and expansion begin, and the soft knee around each, in dB.
const COMPRESS_THRESHOLD: f64 = -24.0;
const EXPAND_THRESHOLD: f64 = -36.0;
const KNEE: f64 = 12.0;
/// The deepest cut expansion makes.
const MAX_EXPANSION: f64 = 60.0;
/// The band envelope's attack and release, in milliseconds.
pub(crate) const BAND_ATTACK_MS: f64 = 3.0;
pub(crate) const BAND_RELEASE_MS: f64 = 100.0;

/// A peak follower with separate attack and release times.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Follower {
    level: f64,
    attack: f64,
    release: f64,
}

impl Follower {
    pub(crate) fn new(rate: f64, attack_ms: f64, release_ms: f64) -> Self {
        let mut follower = Self {
            level: 0.0,
            attack: 0.0,
            release: 0.0,
        };
        follower.set_times(rate, attack_ms, release_ms);
        follower
    }

    pub(crate) fn set_times(&mut self, rate: f64, attack_ms: f64, release_ms: f64) {
        let share = |ms: f64| 1.0 - (-1.0 / (rate * ms.max(0.01) / 1000.0)).exp();
        self.attack = share(attack_ms);
        self.release = share(release_ms);
    }

    #[inline]
    pub(crate) fn follow(&mut self, x: f64) -> f64 {
        let x = x.abs();
        let share = if x > self.level {
            self.attack
        } else {
            self.release
        };
        self.level += share * (x - self.level);
        self.level
    }

    pub(crate) fn level(&self) -> f64 {
        self.level
    }

    pub(crate) fn clear(&mut self) {
        self.level = 0.0;
    }

    pub(crate) fn flush(&mut self) {
        if self.level < 1.0e-25 {
            self.level = 0.0;
        }
    }
}

/// How far `x` dB is past a threshold, through a soft knee `KNEE` dB wide.
fn past(x: f64) -> f64 {
    if x <= -KNEE / 2.0 {
        0.0
    } else if x >= KNEE / 2.0 {
        x
    } else {
        let into = x + KNEE / 2.0;
        into * into / (2.0 * KNEE)
    }
}

/// The gain Dynamics at `amount` (−1 to 1) gives a band whose envelope is at `level`
/// (a linear peak).
pub(crate) fn gain(amount: f64, level: f64) -> f64 {
    let db = 20.0 * level.max(1.0e-9).log10();
    if amount > 0.0 {
        let ratio = 1.0 + 7.0 * amount;
        amplitude(-past(db - COMPRESS_THRESHOLD) * (1.0 - 1.0 / ratio))
    } else if amount < 0.0 {
        let ratio = 1.0 - 3.0 * amount;
        amplitude(-(past(EXPAND_THRESHOLD - db) * (ratio - 1.0)).min(MAX_EXPANSION))
    } else {
        1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db(gain: f64) -> f64 {
        20.0 * gain.log10()
    }

    #[test]
    fn compression_works_above_its_threshold_and_expansion_below_its_own() {
        let level = |dbfs: f64| amplitude(dbfs);
        // Full compression: 8:1, so 24 dB over the threshold comes down by 21 dB.
        assert!((db(gain(1.0, level(0.0))) + 21.0).abs() < 0.01);
        assert!(db(gain(1.0, level(-40.0))) > -0.01);
        assert!(db(gain(0.5, level(0.0))) > db(gain(1.0, level(0.0))));
        // Full expansion: 1:4, so 20 dB under the threshold goes down another 60 dB.
        assert!((db(gain(-1.0, level(-56.0))) + 60.0).abs() < 0.01);
        assert!(db(gain(-1.0, level(-10.0))) > -0.01);
        assert_eq!(gain(0.0, level(-6.0)), 1.0);
    }

    #[test]
    fn the_follower_rises_fast_and_falls_slowly() {
        let mut follower = Follower::new(48_000.0, BAND_ATTACK_MS, BAND_RELEASE_MS);
        for _ in 0..480 {
            follower.follow(1.0);
        }
        assert!(follower.level() > 0.95);
        for _ in 0..4800 {
            follower.follow(0.0);
        }
        let after = follower.level();
        assert!(after > 0.3 && after < 0.4, "{after}");
    }
}
