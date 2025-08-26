use std::{fmt, time::Duration};

use rand::Rng;

pub trait ToDisplay {
    type Formatter<'a>: fmt::Display
    where
        Self: 'a;

    fn display(&self) -> Self::Formatter<'_>;
}

pub struct DurationFmt<'a>(&'a Duration);

impl fmt::Display for DurationFmt<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}ms", self.0.as_millis())
    }
}

impl ToDisplay for Duration {
    type Formatter<'a>
        = DurationFmt<'a>
    where
        Self: 'a;

    fn display(&self) -> Self::Formatter<'_> {
        DurationFmt(self)
    }
}

pub trait MachineRng {
    fn next_election_timeout(&self) -> Duration;
}

/// Generates a random duration in the interval `150..=300` milliseconds.
///
/// Uses [`rand::rngs::ThreadRng`].
pub struct DefaultMachineRng;

impl MachineRng for DefaultMachineRng {
    fn next_election_timeout(&self) -> Duration {
        Duration::from_millis(rand::rng().random_range(150..=300))
    }
}
