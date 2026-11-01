//! Digital Extremes' deterministic interval seed and process-wide RNG.

/// The native interval helper subtracts 2004-01-14T00:00:00Z from a rounded
/// Unix timestamp.
pub(crate) const DE_EPOCH_UNIX: u64 = 0x4004_8680;
pub(crate) const DAILY_INTERVAL_SECONDS: u64 = 86_400;

pub(crate) fn daily_seed(unix_time: u64) -> Option<(u64, u64)> {
    let day_start_unix = unix_time / DAILY_INTERVAL_SECONDS * DAILY_INTERVAL_SECONDS;
    let seed = day_start_unix.checked_sub(DE_EPOCH_UNIX)?;
    Some((day_start_unix, seed))
}

/// Digital Extremes' process-wide seeded RNG used by `SRandomInt` and the
/// corresponding floating-point roll.
pub(crate) struct DeRandom {
    pub(crate) state: u64,
}

impl DeRandom {
    pub(crate) fn seeded(seed: u64) -> Self {
        Self {
            state: if seed == 0 { 1 } else { seed },
        }
    }

    pub(crate) fn sample_30(&mut self) -> u32 {
        self.state = self
            .state
            .wrapping_mul(0x5851_f42d_4c95_7f2d)
            .wrapping_add(0x1405_7b7e_f767_814f);
        (self.state >> 32) as u32 & 0x3fff_ffff
    }

    pub(crate) fn int_inclusive(&mut self, min: usize, max: usize) -> usize {
        if min == max {
            return min;
        }
        let sample = self.sample_30() as usize;
        min + sample % (max - min + 1)
    }

    pub(crate) fn shuffle<T>(&mut self, values: &mut [T]) {
        for len in (2..=values.len()).rev() {
            let index = self.int_inclusive(1, len) - 1;
            values.swap(index, len - 1);
        }
    }
}
