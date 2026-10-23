// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Jareer Abdullah and Concat contributors

//! What a capture helper says, and what it is told.
//!
//! The helpers speak lines. On standard output they announce what they have
//! done - started, paused, resumed, stopped, and where the files went - and
//! on standard error they explain what went wrong, in two registers: a few
//! fixed words for the conditions a caller has to tell apart (a permission
//! that was refused, a microphone that would not open), and prose for
//! everything else.
//!
//! On standard input they take three words: `pause`, `resume`, `stop`. End
//! of input means the same as `stop`, which is what finishes a take when the
//! app quits without being able to ask politely - the helper finalises the
//! file rather than leaving an unplayable one behind.
//!
//! Parsing is by prefix rather than by exact match, because the stopped line
//! carries a path after it and because a helper is allowed to say more than
//! it used to without this file becoming a liar. An unrecognised line is
//! [`Report::Said`]: logged, never fatal.

use std::path::PathBuf;

use crate::error::Error;

/// Why a take ended badly, as a helper reported it.
///
/// A separate type from [`Error`] rather than the same one, so a [`Report`]
/// can be compared and copied: an [`Error`] carries an [`std::io::Error`],
/// which can be neither. [`Failure::into_error`] is the one crossing.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Failure {
    /// Screen recording is not permitted.
    Screen,
    /// The microphone is not permitted.
    Microphone,
    /// The microphone could not be opened.
    MicrophoneUnavailable,
    /// The window being captured is gone.
    WindowGone,
    /// The helper's own words.
    Helper(String),
}

impl Failure {
    /// This failure as the error a caller sees.
    pub fn into_error(self) -> Error {
        match self {
            Failure::Screen => Error::ScreenDenied,
            Failure::Microphone => Error::MicrophoneDenied,
            Failure::MicrophoneUnavailable => Error::MicrophoneUnavailable,
            Failure::WindowGone => Error::WindowGone,
            Failure::Helper(words) => Error::Helper(words),
        }
    }
}

/// One line from a helper.
#[derive(Clone, PartialEq, Debug)]
pub enum Report {
    /// The capture API is running and frames are arriving. A take is not
    /// started until this: the time a take began is taken from here, so the
    /// pointer's path and the picture start together.
    Started,
    /// Frames and samples are no longer being written.
    Paused,
    /// Writing again.
    Resumed,
    /// The file is finalised and complete, at this path.
    Stopped {
        /// Where the picture ended up. The helper echoes back the path it
        /// was given; a take trusts its own path over this one.
        output: PathBuf,
    },
    /// The desktop's sound is finalised at this path.
    SystemAudio {
        /// Where it ended up.
        path: PathBuf,
    },
    /// The microphone is finalised at this path.
    Microphone {
        /// Where it ended up.
        path: PathBuf,
    },
    /// Something went wrong. A take ends here.
    Failed(
        /// What went wrong.
        Failure,
    ),
    /// Anything else the helper said. Logged and otherwise ignored.
    Said(String),
}

impl Report {
    /// Reads one line of a helper's output. The line may come from either
    /// stream: which stream a helper says a thing on has changed between
    /// versions of both of them, so nothing here depends on it.
    pub fn read(line: &str) -> Report {
        let line = line.trim();
        if line.is_empty() {
            return Report::Said(String::new());
        }
        // The fixed words first: a failure that also happens to contain the
        // word "Recording" must not read as progress.
        if let Some(failure) = failure(line) {
            return Report::Failed(failure);
        }
        if line == "Recording started" {
            return Report::Started;
        }
        if line == "Recording paused" {
            return Report::Paused;
        }
        if line == "Recording resumed" {
            return Report::Resumed;
        }
        if let Some(rest) = line.strip_prefix("Recording stopped.") {
            let output = rest
                .trim()
                .strip_prefix("Output path:")
                .map(|path| PathBuf::from(path.trim()))
                .unwrap_or_default();
            return Report::Stopped { output };
        }
        if let Some(rest) = line.strip_prefix("Audio path:") {
            return Report::SystemAudio {
                path: PathBuf::from(rest.trim()),
            };
        }
        if let Some(rest) = line.strip_prefix("Mic path:") {
            return Report::Microphone {
                path: PathBuf::from(rest.trim()),
            };
        }
        Report::Said(line.to_owned())
    }

    /// Whether this line ends the take, one way or the other.
    pub fn is_final(&self) -> bool {
        matches!(self, Report::Stopped { .. } | Report::Failed(_))
    }
}

/// The failure a line names, if it names one. The fixed words are matched
/// whole; `ERROR:` and `Error:` prefixes carry the helper's own words
/// through, since a capture API's complaint is more use on screen than a
/// tidier one of ours.
fn failure(line: &str) -> Option<Failure> {
    match line {
        "SCREEN_RECORDING_PERMISSION_DENIED" => return Some(Failure::Screen),
        "MICROPHONE_PERMISSION_DENIED" => return Some(Failure::Microphone),
        "MICROPHONE_CAPTURE_UNAVAILABLE" => return Some(Failure::MicrophoneUnavailable),
        "WINDOW_UNAVAILABLE" => return Some(Failure::WindowGone),
        _ => {}
    }
    for prefix in ["ERROR:", "Error:"] {
        if let Some(rest) = line.strip_prefix(prefix) {
            return Some(Failure::Helper(rest.trim().to_owned()));
        }
    }
    None
}

/// What a helper is told, on its standard input.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Command {
    /// Stop writing, keep the capture API open.
    Pause,
    /// Write again.
    Resume,
    /// Finalise the file and exit.
    Stop,
}

impl Command {
    /// The word, with its newline. The helpers lowercase what they read, so
    /// the case here is only for the log.
    pub fn line(self) -> &'static str {
        match self {
            Command::Pause => "pause\n",
            Command::Resume => "resume\n",
            Command::Stop => "stop\n",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_is_read() {
        assert_eq!(Report::read("Recording started"), Report::Started);
        assert_eq!(Report::read("Recording paused"), Report::Paused);
        assert_eq!(Report::read("Recording resumed"), Report::Resumed);
    }

    #[test]
    fn the_stopped_line_carries_the_path() {
        let Report::Stopped { output } = Report::read("Recording stopped. Output path: /t/one.mp4")
        else {
            panic!("not read as a stop");
        };
        assert_eq!(output, PathBuf::from("/t/one.mp4"));
    }

    #[test]
    fn a_stop_without_a_path_is_still_a_stop() {
        assert_eq!(
            Report::read("Recording stopped."),
            Report::Stopped {
                output: PathBuf::new()
            }
        );
    }

    #[test]
    fn a_path_with_spaces_survives() {
        let Report::Stopped { output } =
            Report::read("Recording stopped. Output path: /t/my take.mp4")
        else {
            panic!("not read as a stop");
        };
        assert_eq!(output, PathBuf::from("/t/my take.mp4"));
    }

    #[test]
    fn the_sound_files_are_read() {
        assert_eq!(
            Report::read("Audio path: /t/one.system.wav"),
            Report::SystemAudio {
                path: PathBuf::from("/t/one.system.wav")
            }
        );
        assert_eq!(
            Report::read("Mic path: /t/one.mic.wav"),
            Report::Microphone {
                path: PathBuf::from("/t/one.mic.wav")
            }
        );
    }

    #[test]
    fn the_fixed_failures_are_told_apart() {
        assert_eq!(
            Report::read("SCREEN_RECORDING_PERMISSION_DENIED"),
            Report::Failed(Failure::Screen)
        );
        assert_eq!(
            Report::read("MICROPHONE_PERMISSION_DENIED"),
            Report::Failed(Failure::Microphone)
        );
        assert_eq!(
            Report::read("MICROPHONE_CAPTURE_UNAVAILABLE"),
            Report::Failed(Failure::MicrophoneUnavailable)
        );
        assert_eq!(
            Report::read("WINDOW_UNAVAILABLE"),
            Report::Failed(Failure::WindowGone)
        );
        assert!(matches!(
            Report::read("SCREEN_RECORDING_PERMISSION_DENIED").clone(),
            Report::Failed(Failure::Screen)
        ));
    }

    #[test]
    fn a_helper_s_own_words_come_through() {
        let Report::Failed(Failure::Helper(words)) =
            Report::read("ERROR: Failed to initialize WGC capture session")
        else {
            panic!("not read as a failure");
        };
        assert_eq!(words, "Failed to initialize WGC capture session");
    }

    #[test]
    fn anything_else_is_only_said() {
        assert_eq!(
            Report::read("AUDIO_GAPS: droppedBuffers=0 silenceFramesInserted=0"),
            Report::Said("AUDIO_GAPS: droppedBuffers=0 silenceFramesInserted=0".to_owned())
        );
        assert_eq!(Report::read("   "), Report::Said(String::new()));
    }

    #[test]
    fn only_a_stop_or_a_failure_ends_a_take() {
        assert!(!Report::read("Recording started").is_final());
        assert!(!Report::read("Audio path: /t/a.wav").is_final());
        assert!(Report::read("Recording stopped. Output path: /t/a.mp4").is_final());
        assert!(Report::read("WINDOW_UNAVAILABLE").is_final());
    }

    #[test]
    fn a_command_is_one_word_and_a_newline() {
        assert_eq!(Command::Pause.line(), "pause\n");
        assert_eq!(Command::Resume.line(), "resume\n");
        assert_eq!(Command::Stop.line(), "stop\n");
    }
}
