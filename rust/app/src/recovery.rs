// Mirrors the 0.13.3 policy: retry only a wanted connection; notify once per outage.
use std::time::{Duration, Instant};

#[derive(Default)]
pub(super) struct Recovery {
    pub(super) wanted: bool,
    attempts: u8,
    due: Option<Instant>,
    failed_since: Option<Instant>,
    announced: bool,
}
impl Recovery {
    pub(super) fn enable(&mut self) {
        *self = Self {
            wanted: true,
            ..Self::default()
        };
    }
    pub(super) fn stop(&mut self) {
        *self = Self::default();
    }
    pub(super) fn succeeded(&mut self) {
        *self = Self {
            wanted: self.wanted,
            ..Self::default()
        };
    }
    pub(super) fn failed(&mut self, now: Instant) {
        if self.wanted {
            self.failed_since.get_or_insert(now);
            self.due.get_or_insert(now + Duration::from_secs(10));
        }
    }
    pub(super) fn network_changed(&mut self, now: Instant) {
        if self.wanted {
            self.failed(now);
            self.due = Some(now + Duration::from_secs(10));
        }
    }
    pub(super) fn begin(&mut self, now: Instant, online: bool) -> bool {
        if !self.wanted || !online || self.attempts >= 3 || !self.due.is_some_and(|d| now >= d) {
            return false;
        }
        self.attempts += 1;
        self.due = Some(now + Duration::from_secs(u64::from(self.attempts) * 30));
        true
    }
    pub(super) fn notify(&mut self, now: Instant, enabled: bool) -> bool {
        if enabled
            && self.wanted
            && !self.announced
            && self
                .failed_since
                .is_some_and(|s| now.saturating_duration_since(s) >= Duration::from_secs(120))
        {
            self.announced = true;
            return true;
        }
        false
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retries_and_notifications_cannot_restart_a_stopped_connection() {
        let mut recovery = Recovery::default();
        let now = Instant::now();
        let at = |seconds| now + Duration::from_secs(seconds);
        recovery.enable();
        recovery.failed(now);
        recovery.network_changed(now);
        assert!(!recovery.begin(at(9), true));
        assert!(!recovery.begin(at(10), false));
        assert!(recovery.begin(at(10), true));
        recovery.failed(at(11));
        assert!(!recovery.begin(at(39), true));
        assert!(recovery.begin(at(40), true));
        assert!(recovery.begin(at(100), true));
        assert!(!recovery.begin(at(190), true));
        assert!(!recovery.notify(at(119), true));
        assert!(!recovery.notify(at(120), false));
        assert!(recovery.notify(at(130), true));
        assert!(!recovery.notify(at(131), true));
        recovery.succeeded();
        assert!(!recovery.notify(at(400), true));
        recovery.stop();
        recovery.network_changed(at(500));
        assert!(!recovery.begin(at(999), true));
    }
}
