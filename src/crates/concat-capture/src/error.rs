// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Jareer Abdullah and Concat contributors

//! What can go wrong while recording.
//!
//! The variants a caller has to tell apart are the ones with a different
//! thing to say to the person recording: a permission that has to be granted
//! in System Settings is not a helper that is missing from the install, and
//! neither is a window that was closed mid-take. Everything else the helper
//! says is carried through as its own words in [`Error::Helper`], since a
//! capture API's complaint is more use on screen than a tidier one of ours.

use std::path::PathBuf;

/// A take that could not be made, or could not be finished.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// No capture helper on this machine. The paths looked in, so a broken
    /// install can be seen rather than guessed at.
    #[error("no capture helper named {name} (looked in {})", looked_in(looked))]
    NoHelper {
        /// What was looked for.
        name: String,
        /// Where it was looked for, in the order tried.
        looked: Vec<PathBuf>,
    },
    /// The system has not granted screen recording to this app. On macOS
    /// this is Privacy & Security; the grant can lapse when the app is
    /// replaced, so it is a normal thing to meet rather than a broken state.
    #[error("screen recording is not permitted")]
    ScreenDenied,
    /// The system has not granted the microphone to this app.
    #[error("the microphone is not permitted")]
    MicrophoneDenied,
    /// The microphone was asked for and the helper could not open it. The
    /// take still runs, without it - this is the error only when a take was
    /// asked to stand or fall by its sound.
    #[error("the microphone could not be opened")]
    MicrophoneUnavailable,
    /// The window being captured was closed or hidden. Whatever was recorded
    /// up to that point is kept.
    #[error("the window being recorded is gone")]
    WindowGone,
    /// The helper started but never said so, or never finished. What was
    /// being waited for.
    #[error("the capture helper stopped answering while {0}")]
    Unanswered(&'static str),
    /// The helper's own words, for everything it reports that is not one of
    /// the above.
    #[error("{0}")]
    Helper(String),
    /// A file could not be written, read or moved.
    #[error("{0}")]
    Io(#[from] std::io::Error),
}

/// The looked-in paths, for the message. Joined rather than listed so a
/// single-line error stays a single line.
fn looked_in(looked: &[PathBuf]) -> String {
    if looked.is_empty() {
        return "nowhere".to_owned();
    }
    looked
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

/// Shorthand for this crate's results.
pub type Result<T> = std::result::Result<T, Error>;
