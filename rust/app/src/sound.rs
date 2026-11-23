#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Cue {
    Connected,
    Disconnected,
    Failure,
}

pub(super) enum Change {
    Observe,
    Stop,
    Failed,
    Pending,
}

#[derive(Default)]
pub(super) struct Connections {
    route: bool,
    telegram: bool,
}

impl Connections {
    pub(super) fn update(&mut self, route: bool, telegram: bool, change: Change) -> Option<Cue> {
        if matches!(change, Change::Pending) {
            // Compound commands and queued stops report their final state together.
            return None;
        }
        let lost = (self.route && !route) || (self.telegram && !telegram);
        let gained = (!self.route && route) || (!self.telegram && telegram);
        self.route = route;
        self.telegram = telegram;
        match change {
            Change::Failed => Some(Cue::Failure),
            Change::Stop if lost => Some(Cue::Disconnected),
            _ if lost => Some(Cue::Failure),
            _ if gained => Some(Cue::Connected),
            _ => None,
        }
    }
}

#[cfg(windows)]
impl Cue {
    pub(super) fn play(self) {
        use windows_sys::Win32::Media::Audio::{PlaySoundA, SND_ASYNC, SND_MEMORY, SND_NODEFAULT};
        let wav: &'static [u8] = match self {
            Cue::Connected => include_bytes!("../assets/sounds/connected.wav"),
            Cue::Disconnected => include_bytes!("../assets/sounds/disconnected.wav"),
            Cue::Failure => include_bytes!("../assets/sounds/failure.wav"),
        };
        // SND_MEMORY with asynchronous playback requires the bytes to outlive the call.
        unsafe {
            PlaySoundA(
                wav.as_ptr(),
                std::ptr::null_mut(),
                SND_MEMORY | SND_ASYNC | SND_NODEFAULT,
            )
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cues_follow_settled_transitions_and_ignore_repeated_snapshots() {
        let mut state = Connections::default();
        assert_eq!(state.update(false, false, Change::Observe), None);
        assert_eq!(state.update(false, true, Change::Pending), None);
        assert_eq!(
            state.update(true, true, Change::Observe),
            Some(Cue::Connected)
        );
        assert_eq!(state.update(true, true, Change::Observe), None);
        assert_eq!(
            state.update(true, false, Change::Stop),
            Some(Cue::Disconnected)
        );
        assert_eq!(
            state.update(false, false, Change::Observe),
            Some(Cue::Failure)
        );
        assert_eq!(state.update(false, false, Change::Observe), None);
        assert_eq!(
            state.update(false, false, Change::Failed),
            Some(Cue::Failure)
        );
        assert_eq!(state.update(false, false, Change::Stop), None);
    }

    #[test]
    fn cancellation_leaves_the_disconnect_cue_to_the_queued_stop() {
        let mut state = Connections {
            route: true,
            telegram: false,
        };
        assert_eq!(state.update(false, false, Change::Pending), None);
        assert_eq!(
            state.update(false, false, Change::Stop),
            Some(Cue::Disconnected)
        );
        assert_eq!(state.update(false, false, Change::Stop), None);
    }
}
