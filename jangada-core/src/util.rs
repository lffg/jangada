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
pub struct DefaultMachineRng {
    constant: u64,
    factor: u64,
}

impl DefaultMachineRng {
    pub fn new() -> DefaultMachineRng {
        DefaultMachineRng {
            constant: 0,
            factor: 1,
        }
    }

    pub fn with_params(constant: u64, factor: u64) -> DefaultMachineRng {
        DefaultMachineRng { constant, factor }
    }
}

impl Default for DefaultMachineRng {
    fn default() -> Self {
        Self::new()
    }
}

impl MachineRng for DefaultMachineRng {
    fn next_election_timeout(&self) -> Duration {
        Duration::from_millis(self.constant + self.factor * rand::rng().random_range(150..=300))
    }
}
