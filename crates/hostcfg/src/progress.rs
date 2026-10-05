//! The structured build-progress protocol: `xtask --progress json` prints one line per event on
//! stdout, `@archstaler-event ` followed by a JSON object. Every other line is the human log, and a
//! reader must ignore it (never parse its wording).
use serde::{Deserialize, Serialize};

pub const MARKER: &str = "@archstaler-event ";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Start,
    Ok,
    Fail,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    /// A build stage (`config`, `keyring`, `kernel`, `tiny-init`, `uefi-loader`, `bios`, `payload`, `iso`).
    Stage { name: String, state: State },
    /// The ISO is complete: path, size, SHA-256, the profile and config it was built from.
    Done { path: String, size: u64, sha256: String, profile: String, config: String },
    /// The build failed; `message` is the error text.
    Failed { message: String },
}

impl Event {
    /// The line `xtask` prints.
    pub fn line(&self) -> String {
        format!("{MARKER}{}", serde_json::to_string(self).expect("events serialize"))
    }

    /// Parses a line of `xtask` output; `None` for anything that is not an event.
    pub fn parse(line: &str) -> Option<Event> {
        serde_json::from_str(line.strip_prefix(MARKER)?.trim()).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_round_trip_and_log_lines_are_ignored() {
        let e = Event::Stage { name: "kernel".into(), state: State::Start };
        assert_eq!(e.line(), "@archstaler-event {\"event\":\"stage\",\"name\":\"kernel\",\"state\":\"start\"}");
        assert_eq!(Event::parse(&e.line()), Some(e));
        let d = Event::Done { path: "/x.iso".into(), size: 5, sha256: "ab".into(), profile: "large".into(), config: "c.lua".into() };
        assert_eq!(Event::parse(&d.line()), Some(d));
        assert_eq!(Event::parse("payload: kernel 1 -> 2 bytes"), None);
        assert_eq!(Event::parse("@archstaler-event not json"), None);
    }
}
