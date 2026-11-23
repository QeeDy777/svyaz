// Mirrors the 0.13.3 policy: retry only a wanted connection; notify once per outage.
use std::time::{Duration, Instant};

/// NLM can temporarily return no profile or a short-lived set of adapter GUIDs.
#[derive(Default)]
pub(super) struct NetworkProfile {
    confirmed: Option<String>,
    candidate: Option<String>,
}

impl NetworkProfile {
    pub(super) fn new(confirmed: Option<String>) -> Self {
        Self {
            confirmed,
            candidate: None,
        }
    }

    pub(super) fn changed(&mut self, sample: Option<String>) -> bool {
        let Some(id) = sample else {
            self.candidate = None;
            return false;
        };
        if self.confirmed.is_none() {
            self.confirmed = Some(id);
            return false;
        }
        if self.confirmed.as_ref() == Some(&id) {
            self.candidate = None;
            return false;
        }
        if self.candidate.as_ref() == Some(&id) {
            self.confirmed = Some(id);
            self.candidate = None;
            return true;
        }
        self.candidate = Some(id);
        false
    }
}

pub(super) fn different_networks(first: Option<&str>, second: Option<&str>) -> bool {
    matches!((first, second), (Some(first), Some(second)) if first != second)
}

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
    pub(super) fn network_changed(&mut self, now: Instant, running: bool) {
        if self.wanted && !running {
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
    fn missing_samples_and_an_unchanged_profile_after_a_long_operation_stay_quiet() {
        let mut profile = NetworkProfile::default();
        for id in [
            None,
            Some("wifi"),
            None,
            Some("wifi"),
            Some("vpn"),
            Some("wifi"),
            Some("wifi"),
        ] {
            assert!(!profile.changed(id.map(str::to_owned)));
        }
    }

    #[test]
    fn a_confirmed_network_change_is_reported_once_in_each_direction() {
        let mut profile = NetworkProfile::new(Some("wifi".into()));
        let events: Vec<_> = ["mobile", "mobile", "mobile", "wifi", "wifi", "wifi"]
            .map(|id| profile.changed(Some(id.into())))
            .into();
        assert_eq!(events, [false, true, false, false, true, false]);
    }

    #[test]
    fn finishing_a_connection_replaces_the_previous_poll_baseline() {
        let mut profile = NetworkProfile::new(Some("mobile".into()));
        assert!(!profile.changed(Some("mobile".into())));
        assert!(!different_networks(Some("mobile"), None));
        assert!(!different_networks(None, Some("wifi")));
        assert!(!different_networks(Some("wifi"), Some("wifi")));
        assert!(different_networks(Some("wifi"), Some("mobile")));
    }

    #[test]
    fn a_changed_profile_does_not_schedule_a_probe_or_restart_of_a_running_service() {
        let mut recovery = Recovery::default();
        let now = Instant::now();
        recovery.enable();
        recovery.succeeded();
        recovery.network_changed(now, true);
        assert!(!recovery.begin(now + Duration::from_secs(600), true));
        assert!(!recovery.notify(now + Duration::from_secs(600), true));
    }
    #[test]
    fn retries_and_notifications_cannot_restart_a_stopped_connection() {
        let mut recovery = Recovery::default();
        let now = Instant::now();
        let at = |seconds| now + Duration::from_secs(seconds);
        recovery.enable();
        recovery.failed(now);
        recovery.network_changed(now, false);
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
        recovery.network_changed(at(500), false);
        assert!(!recovery.begin(at(999), true));
    }
}
