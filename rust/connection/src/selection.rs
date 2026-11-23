use crate::ProbeResult;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    FirstPassing,
    Compare,
    Full,
}

impl Mode {
    /// A new machine has to evaluate every strategy, even with a manual preference.
    pub fn for_connection(last_strategy: Option<&str>, manual: bool, full_scan: bool) -> Self {
        if full_scan || last_strategy.is_none_or(|s| s.trim().is_empty()) {
            Self::Full
        } else if manual {
            Self::FirstPassing
        } else {
            Self::Compare
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum StartError {
    Candidate(String),
    Conflict(String),
}

#[derive(Debug, PartialEq, Eq)]
pub enum SelectionError {
    RequiresManualRetry(String),
    Cancelled,
    Runner(String),
}

pub trait Runner {
    fn start(&mut self, strategy: &str, cancel: &AtomicBool) -> Result<(), StartError>;
    /// Wait for this runner's previous process to terminate before returning.
    fn stop(&mut self) -> Result<(), String>;
    fn is_running(&self) -> bool;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Quality {
    pub median_ms: u64,
    pub spread_ms: u64,
}
impl Quality {
    fn from_samples(mut samples: [u64; 3]) -> Self {
        samples.sort_unstable();
        Self {
            median_ms: samples[1],
            spread_ms: samples[2] - samples[0],
        }
    }
    fn score(self) -> u128 {
        u128::from(self.median_ms) + u128::from(self.spread_ms)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    Starting {
        position: usize,
        total: usize,
        confirmation: bool,
    },
    StartFailed(String),
    Checked {
        pass: usize,
        passes: usize,
        result: ProbeResult,
        confirmed: Option<Quality>,
    },
}

struct Engine<'a, R, P, F> {
    runner: &'a mut R,
    cancel: &'a AtomicBool,
    probe: P,
    report: F,
    total: usize,
}

impl<R: Runner, P: FnMut() -> ProbeResult, F: FnMut(&str, Outcome)> Engine<'_, R, P, F> {
    fn guard(&self) -> Result<(), SelectionError> {
        if self.cancel.load(Ordering::Acquire) {
            Err(SelectionError::Cancelled)
        } else {
            Ok(())
        }
    }

    fn evaluate(
        &mut self,
        strategy: &str,
        position: usize,
        passes: usize,
        confirmation: Option<Quality>,
    ) -> Result<Option<[u64; 3]>, SelectionError> {
        self.guard()?;
        self.runner.stop().map_err(SelectionError::Runner)?;
        self.guard()?;
        (self.report)(
            strategy,
            Outcome::Starting {
                position,
                total: self.total,
                confirmation: confirmation.is_some(),
            },
        );
        match self.runner.start(strategy, self.cancel) {
            Ok(()) => (),
            Err(StartError::Candidate(reason)) => {
                self.guard()?;
                (self.report)(strategy, Outcome::StartFailed(reason));
                return Ok(None);
            }
            Err(StartError::Conflict(reason)) => {
                return Err(SelectionError::RequiresManualRetry(reason));
            }
        }
        let mut samples = [0; 3];
        for (index, sample) in samples.iter_mut().enumerate().take(passes) {
            self.guard()?;
            let result = (self.probe)();
            self.guard()?;
            let elapsed = result.sample_ms();
            let confirmed = confirmation.filter(|_| elapsed.is_some() && index + 1 == passes);
            (self.report)(
                strategy,
                Outcome::Checked {
                    pass: index + 1,
                    passes,
                    result,
                    confirmed,
                },
            );
            let Some(elapsed) = elapsed else {
                return Ok(None);
            };
            if !self.runner.is_running() {
                return Ok(None);
            }
            *sample = elapsed;
        }
        Ok(Some(samples))
    }

    fn run(
        &mut self,
        strategies: &[String],
        preferred: Option<&str>,
        mode: Mode,
    ) -> Result<Option<String>, SelectionError> {
        let mut ordered: Vec<&str> = strategies.iter().map(String::as_str).collect();
        ordered.sort_by_key(|s| Some(*s) != preferred);
        let mut candidates = Vec::new();
        let mut additional = 0;
        for (index, strategy) in ordered.into_iter().enumerate() {
            if mode != Mode::Full && !candidates.is_empty() {
                if additional == 6 {
                    break;
                }
                additional += 1;
            }
            let passes = if mode == Mode::FirstPassing { 2 } else { 3 };
            if let Some(samples) = self.evaluate(strategy, index + 1, passes, None)? {
                if mode == Mode::FirstPassing {
                    return Ok(Some(strategy.to_owned()));
                }
                candidates.push((strategy, index + 1, Quality::from_samples(samples)));
                if mode != Mode::Full && candidates.len() == 3 {
                    break;
                }
            }
        }
        candidates.sort_by_key(|(_, _, quality)| quality.score());
        for (strategy, position, quality) in candidates {
            if self
                .evaluate(strategy, position, 2, Some(quality))?
                .is_some()
            {
                return Ok(Some(strategy.to_owned()));
            }
        }
        Ok(None)
    }
}

/// Synchronous worker entry point. Only a twice-confirmed winner remains running.
pub fn select<R: Runner>(
    runner: &mut R,
    strategies: &[String],
    preferred: Option<&str>,
    mode: Mode,
    cancel: &AtomicBool,
    probe: impl FnMut() -> ProbeResult,
    report: impl FnMut(&str, Outcome),
) -> Result<Option<String>, SelectionError> {
    let mut engine = Engine {
        runner,
        cancel,
        probe,
        report,
        total: strategies.len(),
    };
    let result = engine.run(strategies, preferred, mode);
    if !matches!(result, Ok(Some(_))) {
        engine.runner.stop().map_err(SelectionError::Runner)?;
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::CheckResult;
    use std::{cell::RefCell, collections::VecDeque, rc::Rc};

    #[derive(Default)]
    struct Fake {
        starts: Vec<String>,
        active: Option<String>,
        blocked: Option<String>,
    }
    impl Runner for Fake {
        fn start(&mut self, strategy: &str, _: &AtomicBool) -> Result<(), StartError> {
            assert!(self.active.is_none(), "Previous process still running");
            self.starts.push(strategy.into());
            if self.blocked.as_deref() == Some(strategy) {
                return Err(StartError::Conflict("external process".into()));
            }
            self.active = Some(strategy.into());
            Ok(())
        }
        fn stop(&mut self) -> Result<(), String> {
            self.active = None;
            Ok(())
        }
        fn is_running(&self) -> bool {
            self.active.is_some()
        }
    }
    fn sample(ms: Option<u64>) -> ProbeResult {
        let one = ms.map_or_else(
            || CheckResult::Failed("no response".into()),
            |elapsed_ms| CheckResult::Success { elapsed_ms },
        );
        ProbeResult {
            api: one,
            gateway: CheckResult::Skipped,
            voice: CheckResult::Skipped,
            youtube: CheckResult::Skipped,
            google_video: CheckResult::Skipped,
        }
    }

    #[test]
    fn first_connection_visits_all_then_reconfirms_next_after_winner_fails() {
        assert_eq!(Mode::for_connection(None, true, false), Mode::Full);
        let strategies: Vec<_> = ["a", "b", "c", "d"].map(String::from).into();
        let mut runner = Fake::default();
        let mut sequence: VecDeque<_> =
            [2, 2, 2, 10, 10, 10, 60, 1, 1, 20, 20, 20, 0, 10, 10].into();
        let selected = select(
            &mut runner,
            &strategies,
            Some("b"),
            Mode::Full,
            &AtomicBool::new(false),
            || {
                let v = sequence.pop_front().unwrap();
                sample((v != 0).then_some(v))
            },
            |_, _| {},
        )
        .unwrap();
        assert_eq!(runner.starts, ["b", "a", "c", "d", "b", "a"]);
        assert_eq!(selected, runner.active);
        assert_eq!(selected.as_deref(), Some("a"));
        assert!(sequence.is_empty());
    }

    #[test]
    fn manual_stops_at_first_double_pass_and_quick_compare_bounds_candidates() {
        let strategies: Vec<_> = (0..11).map(|i| i.to_string()).collect();
        let mut runner = Fake::default();
        let cancel = AtomicBool::new(false);
        select(
            &mut runner,
            &strategies,
            Some("3"),
            Mode::FirstPassing,
            &cancel,
            || sample(Some(50)),
            |_, _| {},
        )
        .unwrap();
        assert_eq!(runner.starts, ["3"]);
        let mut runner = Fake::default();
        let mut attempts = 0;
        select(
            &mut runner,
            &strategies,
            None,
            Mode::Compare,
            &cancel,
            || {
                attempts += 1;
                sample(if attempts <= 3 || attempts > 9 {
                    Some(5)
                } else {
                    None
                })
            },
            |_, _| {},
        )
        .unwrap();
        assert_eq!(runner.starts, ["0", "1", "2", "3", "4", "5", "6", "0"]);
        assert_eq!(runner.active.as_deref(), Some("0"));
    }

    #[test]
    fn cancel_and_external_conflict_leave_no_owned_process() {
        let names = vec!["a".into(), "b".into()];
        let cancel = AtomicBool::new(false);
        let observed = Rc::new(RefCell::new(0));
        let counter = observed.clone();
        let mut runner = Fake::default();
        let result = select(
            &mut runner,
            &names,
            None,
            Mode::Full,
            &cancel,
            || {
                cancel.store(true, Ordering::Release);
                sample(Some(1))
            },
            |_, event| {
                if matches!(event, Outcome::Checked { .. }) {
                    *counter.borrow_mut() += 1;
                }
            },
        );
        assert_eq!(result, Err(SelectionError::Cancelled));
        assert!(runner.active.is_none());
        assert_eq!(*observed.borrow(), 0);
        let mut runner = Fake {
            blocked: Some("a".into()),
            ..Default::default()
        };
        assert!(matches!(
            select(
                &mut runner,
                &names,
                None,
                Mode::Full,
                &AtomicBool::new(false),
                || sample(Some(2)),
                |_, _| {}
            ),
            Err(SelectionError::RequiresManualRetry(_))
        ));
        assert_eq!(runner.starts, ["a"]);
        assert!(runner.active.is_none());
    }
}
