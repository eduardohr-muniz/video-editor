// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Jareer Abdullah and Concat contributors

//! Finding the capture helpers on this machine.
//!
//! Three small binaries, each owning one platform API, shipped beside the
//! app rather than linked into it (see the crate's documentation for why).
//! Where "beside" is depends on how the app was installed, so they are
//! looked for in a fixed order, nearest first:
//!
//! 1. `CONCAT_CAPTURE_HELPERS`, when it is set - a folder named by hand,
//!    which is how a development tree points at a build of them.
//! 2. The folder the running executable is in.
//! 3. `../Resources/helpers`, from there: inside a macOS app bundle the
//!    executable is in `Contents/MacOS` and anything that is not code
//!    signed as part of it belongs in `Contents/Resources`.
//! 4. `helpers/`, from there: the layout of the Windows install and the
//!    Linux tarball.
//!
//! The first readable file wins, and a helper that is nowhere is an
//! [`Error::NoHelper`] naming every path tried, because the alternative - a
//! recorder that silently does nothing - is the bug that takes an afternoon
//! to find.
//!
//! Each role accepts more than one file name. The second name in each list
//! is the one the helper carries in the Recordly tree these were taken from,
//! so pointing `CONCAT_CAPTURE_HELPERS` at that tree's `electron/native/bin`
//! works without renaming anything - which is how this crate was brought up
//! before the helpers were vendored.

use std::path::{Path, PathBuf};

use crate::error::{Error, Result};

/// Which helper. Each is one platform API and nothing else.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Helper {
    /// The screen recorder: ScreenCaptureKit on macOS, Windows Graphics
    /// Capture on Windows. Writes the picture, and the sound when asked.
    Screen,
    /// The pointer monitor: reports a click and a change of cursor shape as
    /// they happen. Not the pointer's position, which is sampled in-process;
    /// see [`crate::cursor`].
    Pointer,
    /// Lists the windows that can be captured, for the picker.
    Windows,
}

impl Helper {
    /// The file names this helper may carry, in the order preferred.
    pub fn names(self) -> &'static [&'static str] {
        match (self, cfg!(windows)) {
            (Helper::Screen, false) => &[
                "concat-screencapturekit-helper",
                "recordly-screencapturekit-helper",
            ],
            (Helper::Screen, true) => &["concat-wgc-capture.exe", "wgc-capture.exe"],
            (Helper::Pointer, false) => {
                &["concat-cursor-monitor", "recordly-native-cursor-monitor"]
            }
            (Helper::Pointer, true) => &["concat-cursor-monitor.exe", "cursor-monitor.exe"],
            (Helper::Windows, false) => &["concat-window-list", "recordly-window-list"],
            (Helper::Windows, true) => &["concat-window-list.exe", "openscreen-window-list.exe"],
        }
    }

    /// Where this helper is, or every path that was tried.
    pub fn locate(self) -> Result<PathBuf> {
        let mut looked = Vec::new();
        for folder in folders() {
            for name in self.names() {
                let candidate = folder.join(name);
                if is_runnable(&candidate) {
                    log::debug!("capture helper {name} at {}", candidate.display());
                    return Ok(candidate);
                }
                looked.push(candidate);
            }
        }
        Err(Error::NoHelper {
            name: self.names()[0].to_owned(),
            looked,
        })
    }
}

/// The folders searched, nearest first. A folder that does not exist is
/// still listed: it is what an install is missing, and saying so is the
/// point of the error.
fn folders() -> Vec<PathBuf> {
    let mut folders = Vec::new();
    if let Some(named) = std::env::var_os("CONCAT_CAPTURE_HELPERS") {
        folders.push(PathBuf::from(named));
    }
    if let Some(beside) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
    {
        folders.push(beside.clone());
        // Contents/MacOS -> Contents/Resources/helpers.
        folders.push(beside.join("../Resources/helpers"));
        folders.push(beside.join("helpers"));
    }
    folders
}

/// Whether this path is a file something can be run from. The executable bit
/// is not checked: a helper that is present and not executable is a broken
/// install that should say "could not start" with the system's own reason,
/// not "no helper", which sends the reader looking in the wrong place.
fn is_runnable(path: &Path) -> bool {
    path.is_file()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_role_offers_a_name_on_this_platform() {
        for helper in [Helper::Screen, Helper::Pointer, Helper::Windows] {
            assert!(!helper.names().is_empty(), "{helper:?} has no name");
            for name in helper.names() {
                assert_eq!(
                    name.ends_with(".exe"),
                    cfg!(windows),
                    "{name} is not spelled for this platform"
                );
            }
        }
    }

    #[test]
    fn the_concat_name_is_preferred_over_the_vendored_one() {
        for helper in [Helper::Screen, Helper::Pointer, Helper::Windows] {
            assert!(
                helper.names()[0].starts_with("concat-"),
                "{helper:?} prefers a name that is not ours"
            );
        }
    }

    #[test]
    fn a_helper_that_is_nowhere_names_the_paths_tried() {
        // Nothing of ours is in a folder that cannot exist, so this is the
        // missing-helper path on every machine.
        temp_env("CONCAT_CAPTURE_HELPERS", "/concat-capture-nowhere");
        let Err(Error::NoHelper { name, looked }) = Helper::Screen.locate() else {
            panic!("a helper was found in a folder that does not exist");
        };
        assert_eq!(name, Helper::Screen.names()[0]);
        assert!(
            looked
                .iter()
                .any(|path| path.starts_with("/concat-capture-nowhere")),
            "the named folder was not tried: {looked:?}"
        );
    }

    /// The tests in this module run in one process, and only this one sets
    /// the variable; `std::env::set_var` is unsafe from 2024 because of the
    /// threads it would race with elsewhere.
    fn temp_env(key: &str, value: &str) {
        unsafe { std::env::set_var(key, value) };
    }
}
