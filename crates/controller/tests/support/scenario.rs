use std::time::Duration;

pub fn run<T>(_name: &str, _limit: Duration, scenario: impl FnOnce() -> T) -> T {
    scenario()
}

pub fn progress(_marker: impl Into<String>) {}
