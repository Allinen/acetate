//! When a listen counts as a play (reported with `POST /api/tracks/{id}/played`): after
//! half the track or 4 minutes of it, whichever comes first, as Navidrome counts them.
//! Only time actually listened counts: a seek forward doesn't, and pausing stops the
//! clock. Each listen is reported once; playing the track again (from its start) is a
//! new listen.

use std::time::Duration;

use uuid::Uuid;

const AT_MOST: Duration = Duration::from_secs(240);

#[derive(Default)]
pub struct Listen {
    track: Option<Uuid>,
    last_position: Duration,
    listened: Duration,
    reported: bool,
}

impl Listen {
    /// Called regularly with what plays. Returns the track once its listen counts.
    pub fn update(&mut self, track: Option<Uuid>, length: Option<Duration>, position: Duration, paused: bool) -> Option<Uuid> {
        let Some(id) = track else {
            self.track = None;
            return None;
        };
        // Another track, or this one again from its start: a new listen.
        let again = position + Duration::from_secs(2) < self.last_position && position < Duration::from_secs(5);
        if self.track != Some(id) || again {
            *self = Listen { track: Some(id), last_position: position, ..Listen::default() };
            return None;
        }
        let step = position.saturating_sub(self.last_position);
        self.last_position = position;
        // Small steps are listening; big ones (or backwards) are seeks.
        if !paused && step < Duration::from_secs(2) {
            self.listened += step;
        }
        let needed = match length {
            Some(length) if length > Duration::ZERO => (length / 2).min(AT_MOST),
            _ => AT_MOST,
        };
        if !self.reported && self.listened >= needed {
            self.reported = true;
            return Some(id);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn secs(s: f32) -> Duration {
        Duration::from_secs_f32(s)
    }

    /// Plays `track` from `from` to `to` seconds in quarter-second steps; returns what was
    /// reported.
    fn play(listen: &mut Listen, track: Uuid, length: f32, from: f32, to: f32) -> Vec<Uuid> {
        let mut reported = Vec::new();
        let mut at = from;
        while at <= to {
            reported.extend(listen.update(Some(track), Some(secs(length)), secs(at), false));
            at += 0.25;
        }
        reported
    }

    #[test]
    fn half_the_track_or_four_minutes() {
        let t = Uuid::from_u128(1);
        let mut listen = Listen::default();
        assert!(play(&mut listen, t, 100.0, 0.0, 49.0).is_empty());
        assert_eq!(play(&mut listen, t, 100.0, 49.25, 60.0), [t], "once, at half");
        assert!(play(&mut listen, t, 100.0, 60.25, 99.0).is_empty(), "not twice");

        let long = Uuid::from_u128(2);
        assert_eq!(play(&mut listen, long, 3600.0, 0.0, 241.0), [long], "4 minutes of an hour-long mix");
    }

    #[test]
    fn seeking_ahead_doesnt_count_and_a_replay_does() {
        let t = Uuid::from_u128(1);
        let mut listen = Listen::default();
        assert!(play(&mut listen, t, 100.0, 0.0, 5.0).is_empty());
        // Jumps to 90 s: only the 10 s listened after that count (15 s in all).
        assert!(play(&mut listen, t, 100.0, 90.0, 100.0).is_empty());
        // From the start again: a new listen, which counts at 50 s.
        assert_eq!(play(&mut listen, t, 100.0, 0.0, 51.0), [t]);
    }
}
