use crate::{Error, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Eq)]
struct Epoch {
    generation: i64,
    pending: bool,
    delay: u32,
}

struct Entry {
    epoch: Epoch,
    deadline: Instant,
}

#[derive(Default)]
struct BudgetClock {
    entries: HashMap<PathBuf, Entry>,
}

impl BudgetClock {
    fn remaining(&mut self, path: &Path, epoch: Epoch, now: Instant) -> Result<u32> {
        if epoch.delay == 0 {
            return Ok(0);
        }
        if epoch.delay == u32::MAX {
            return Err(Error::Internal(
                "paused provider interval has no monotonic deadline",
            ));
        }
        if let Some(entry) = self.entries.get(path).filter(|entry| entry.epoch == epoch) {
            let remaining = entry.deadline.saturating_duration_since(now);
            return Ok(u32::try_from(remaining.as_secs())
                .map_err(|_| Error::Internal("provider monotonic interval exceeds its bound"))?
                + u32::from(remaining.subsec_nanos() != 0));
        }
        if !self.entries.contains_key(path) && self.entries.len() >= 128 {
            self.entries.retain(|_, entry| entry.deadline > now);
            if self.entries.len() >= 128 {
                return Err(Error::Internal("provider monotonic clock capacity reached"));
            }
        }
        let deadline = now
            .checked_add(Duration::from_secs(u64::from(epoch.delay)))
            .ok_or(Error::Internal(
                "provider monotonic deadline is unsupported",
            ))?;
        self.entries
            .insert(path.to_owned(), Entry { epoch, deadline });
        Ok(epoch.delay)
    }
}

/// A new process waits the full saved interval when it first sees an epoch.
/// This can extend backoff after restart, but a wall-clock jump cannot shorten
/// it. Reopened SQLite connections share the same bounded process clock.
pub(super) fn remaining(path: &Path, generation: i64, pending: bool, delay: u32) -> Result<u32> {
    static CLOCK: OnceLock<Mutex<BudgetClock>> = OnceLock::new();
    CLOCK
        .get_or_init(|| Mutex::new(BudgetClock::default()))
        .lock()
        .map_err(|_| Error::Internal("provider monotonic clock is poisoned"))?
        .remaining(
            path,
            Epoch {
                generation,
                pending,
                delay,
            },
            Instant::now(),
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_clock_restarts_conservatively_rounds_up_and_separates_epochs() -> Result<()> {
        let mut clock = BudgetClock::default();
        let path = Path::new("state.db");
        let now = Instant::now();
        let epoch = Epoch {
            generation: 1,
            pending: false,
            delay: 120,
        };
        assert_eq!(clock.remaining(path, epoch, now)?, 120);
        assert_eq!(
            clock.remaining(path, epoch, now + Duration::from_millis(500))?,
            120
        );
        assert_eq!(
            clock.remaining(path, epoch, now + Duration::from_secs(119))?,
            1
        );
        assert_eq!(
            clock.remaining(path, epoch, now + Duration::from_secs(120))?,
            0
        );
        assert_eq!(
            clock.remaining(path, epoch, now + Duration::from_secs(121))?,
            0
        );
        assert_eq!(
            clock.remaining(Path::new("other.db"), epoch, now + Duration::from_secs(121))?,
            120
        );
        assert_eq!(
            clock.remaining(path, epoch, now + Duration::from_secs(121))?,
            0
        );
        let mut restarted = BudgetClock::default();
        assert_eq!(
            restarted.remaining(path, epoch, now + Duration::from_secs(121))?,
            120
        );
        let pending = Epoch {
            generation: 2,
            pending: true,
            delay: 60,
        };
        assert_eq!(clock.remaining(path, pending, now)?, 60);
        let completed = Epoch {
            pending: false,
            ..pending
        };
        assert_eq!(
            clock.remaining(path, completed, now + Duration::from_secs(50))?,
            60
        );
        let next = Epoch {
            generation: 3,
            ..completed
        };
        assert_eq!(
            clock.remaining(path, next, now + Duration::from_secs(100))?,
            60
        );
        Ok(())
    }

    #[test]
    fn catalog_clock_capacity_is_bounded_and_expired_entries_can_be_reclaimed() -> Result<()> {
        let mut clock = BudgetClock::default();
        let now = Instant::now();
        let epoch = Epoch {
            generation: 1,
            pending: false,
            delay: 1,
        };
        for index in 0..128 {
            assert_eq!(
                clock.remaining(Path::new(&format!("{index}.db")), epoch, now)?,
                1
            );
        }
        assert!(clock.remaining(Path::new("extra.db"), epoch, now).is_err());
        assert_eq!(
            clock.remaining(Path::new("extra.db"), epoch, now + Duration::from_secs(1))?,
            1
        );
        assert_eq!(clock.entries.len(), 1);
        Ok(())
    }
}
