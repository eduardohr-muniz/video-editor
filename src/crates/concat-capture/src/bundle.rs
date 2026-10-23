// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Jareer Abdullah and Concat contributors

//! What a take leaves on disk.
//!
//! One folder per take, holding the picture, each sound as its own file, the
//! pointer's path, and a manifest naming them:
//!
//! ```text
//! 2026-10-22 18-04-11/
//!   screen.mp4
//!   screen.mp4.cursor.json
//!   system.wav
//!   microphone.wav
//!   take.json
//! ```
//!
//! A folder rather than the flat `recording-<millis>.mp4`,
//! `recording-<millis>.system.wav` naming a recorder usually grows into,
//! because a take is four files that only mean anything together: moving it,
//! copying it to another machine or throwing it away should be one gesture,
//! and an import that half-finds its sound is a bug waiting to be filed.
//!
//! The sound is kept apart rather than muxed into the picture on purpose.
//! The desktop and the microphone want different treatment almost every
//! time - the room noise cleaned off one, the other left alone - and the
//! editor can only offer that if the take did not mix them. They rejoin as
//! two audio streams of one media item when the bundle is imported.
//!
//! Every path in the manifest is relative to the folder, so a bundle that
//! is moved still reads.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::source::{Area, Source};

/// The picture's file name inside a bundle.
pub const VIDEO: &str = "screen.mp4";
/// The desktop sound's file name.
pub const SYSTEM_AUDIO: &str = "system.wav";
/// The microphone's file name.
pub const MICROPHONE: &str = "microphone.wav";
/// The manifest's file name.
pub const MANIFEST: &str = "take.json";

/// A take on disk.
#[derive(Clone, PartialEq, Debug)]
pub struct Bundle {
    /// The folder everything is in.
    pub folder: PathBuf,
    /// What the take knows about itself.
    pub manifest: Manifest,
}

/// What a take knows about itself. Written as `take.json`.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    /// The shape of this file. One, so far.
    pub version: u32,
    /// What was captured, in words, for the library.
    pub source: String,
    /// The rectangle captured, in the platform's screen units. What the
    /// pointer's path was measured against; the editor needs it to place a
    /// zoom in a frame whose pixels are a different size.
    pub area: Area,
    /// Frames a second asked of the capture API.
    pub fps: u32,
    /// How long the take ran, with pauses taken out, in milliseconds. What
    /// the recorder measured: the file's own duration is what the editor
    /// trusts, and the two differ by the last frame's length.
    pub duration_ms: f64,
    /// When the take began, as seconds since the epoch. For sorting a
    /// library without stat-ing every folder.
    pub began_at: f64,
    /// The picture, relative to the folder.
    pub video: String,
    /// The desktop's sound, relative to the folder, when there is any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_audio: Option<String>,
    /// The microphone, relative to the folder, when there is any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub microphone: Option<String>,
    /// How many pointer samples were collected. Zero means there is no
    /// `.cursor.json`, and so no automatic zoom to be had from this take.
    #[serde(default)]
    pub pointer_samples: usize,
}

impl Manifest {
    /// The shape this build writes.
    pub const VERSION: u32 = 1;

    /// A manifest for a take about to be made.
    pub fn new(source: &Source, fps: u32) -> Manifest {
        Manifest {
            version: Manifest::VERSION,
            source: source.describe(),
            area: source.area(),
            fps,
            duration_ms: 0.0,
            began_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|since| since.as_secs_f64())
                .unwrap_or_default(),
            video: VIDEO.to_owned(),
            system_audio: None,
            microphone: None,
            pointer_samples: 0,
        }
    }
}

impl Bundle {
    /// A new, empty folder for a take, under `recordings`, named for the
    /// moment it was made. A second take in the same second gets a suffix
    /// rather than overwriting the first.
    pub fn create(recordings: &Path, source: &Source, fps: u32) -> std::io::Result<Bundle> {
        let stem = folder_name();
        let mut folder = recordings.join(&stem);
        let mut attempt = 2;
        while folder.exists() {
            folder = recordings.join(format!("{stem} ({attempt})"));
            attempt += 1;
            if attempt > 100 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    "a hundred takes in one second",
                ));
            }
        }
        std::fs::create_dir_all(&folder)?;
        Ok(Bundle {
            folder,
            manifest: Manifest::new(source, fps),
        })
    }

    /// Reads a bundle's manifest. A folder with no manifest, or one that
    /// cannot be read, is not a bundle.
    pub fn open(folder: &Path) -> Option<Bundle> {
        let text = std::fs::read_to_string(folder.join(MANIFEST)).ok()?;
        let manifest: Manifest = serde_json::from_str(&text).ok()?;
        Some(Bundle {
            folder: folder.to_path_buf(),
            manifest,
        })
    }

    /// Where the picture is.
    pub fn video(&self) -> PathBuf {
        self.folder.join(&self.manifest.video)
    }

    /// Where the desktop's sound is, when the take has any.
    pub fn system_audio(&self) -> Option<PathBuf> {
        self.manifest
            .system_audio
            .as_ref()
            .map(|name| self.folder.join(name))
    }

    /// Where the microphone is, when the take has any.
    pub fn microphone(&self) -> Option<PathBuf> {
        self.manifest
            .microphone
            .as_ref()
            .map(|name| self.folder.join(name))
    }

    /// Where the pointer's path is, whether or not it exists.
    pub fn pointer(&self) -> PathBuf {
        crate::cursor::Telemetry::path_beside(&self.video())
    }

    /// Writes the manifest. Called once when the take finishes; a bundle
    /// without one is a take that was interrupted, and is skipped by the
    /// library rather than shown as a broken row.
    pub fn write_manifest(&self) -> std::io::Result<()> {
        let text = serde_json::to_string_pretty(&self.manifest)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        std::fs::write(self.folder.join(MANIFEST), text)
    }

    /// Whether the take has a picture worth keeping: a file with something
    /// in it. A helper that failed before the first frame leaves an empty
    /// file behind, and that is not a take.
    pub fn has_picture(&self) -> bool {
        std::fs::metadata(self.video()).is_ok_and(|file| file.len() > 0)
    }

    /// Throws the folder away. For a take that was cancelled, or one whose
    /// helper never produced a frame.
    pub fn discard(&self) -> std::io::Result<()> {
        match std::fs::remove_dir_all(&self.folder) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            other => other,
        }
    }
}

/// The bundles under a folder, newest first. Anything that is not a bundle
/// is passed over in silence: the folder is the one a person can open, and
/// what else they leave in it is their business.
pub fn library(recordings: &Path) -> Vec<Bundle> {
    let Ok(entries) = std::fs::read_dir(recordings) else {
        return Vec::new();
    };
    let mut bundles: Vec<Bundle> = entries
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter_map(|entry| Bundle::open(&entry.path()))
        .collect();
    bundles.sort_by(|left, right| right.manifest.began_at.total_cmp(&left.manifest.began_at));
    bundles
}

/// `2026-10-22 18-04-11`: sortable, and a name a file manager will show
/// without escaping anything. Built from the wall clock by hand - a date
/// library for one format string is a dependency this crate does not need.
fn folder_name() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or_default();
    let (year, month, day, hour, minute, second) = civil(now as i64);
    format!("{year:04}-{month:02}-{day:02} {hour:02}-{minute:02}-{second:02}")
}

/// Seconds since the epoch as a UTC date and time. Howard Hinnant's
/// `civil_from_days`, which is the shortest correct way to do this without a
/// calendar library: days are shifted onto an era of 400 years, whose length
/// in days is exact, and the month is recovered from the day of the year.
fn civil(seconds: i64) -> (i64, u32, u32, u32, u32, u32) {
    let days = seconds.div_euclid(86_400);
    let rest = seconds.rem_euclid(86_400);
    let (hour, minute, second) = (rest / 3600, (rest % 3600) / 60, rest % 60);

    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = if month <= 2 { year + 1 } else { year };
    (
        year,
        month as u32,
        day as u32,
        hour as u32,
        minute as u32,
        second as u32,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source() -> Source {
        Source::Screen {
            id: 1,
            area: Area::sized(1920.0, 1080.0),
        }
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "concat-capture-{}-{}-{name}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|since| since.as_nanos())
                .unwrap_or_default()
        ));
        std::fs::create_dir_all(&dir).expect("a temporary folder");
        dir
    }

    #[test]
    fn the_epoch_is_the_first_of_january_1970() {
        assert_eq!(civil(0), (1970, 1, 1, 0, 0, 0));
    }

    #[test]
    fn a_leap_day_is_a_leap_day() {
        // 2024-02-29 12:34:56 UTC.
        assert_eq!(civil(1_709_210_096), (2024, 2, 29, 12, 34, 56));
    }

    #[test]
    fn a_century_that_is_not_a_leap_year() {
        // 1900-03-01 00:00:00 UTC, the day after a February that had 28 days.
        assert_eq!(civil(-2_203_891_200), (1900, 3, 1, 0, 0, 0));
    }

    #[test]
    fn the_folder_name_sorts_by_time() {
        let name = folder_name();
        assert_eq!(name.len(), 19, "{name}");
        assert!(name.starts_with("20"), "{name}");
    }

    #[test]
    fn a_take_gets_its_own_folder_even_in_the_same_second() {
        let root = scratch("folders");
        let first = Bundle::create(&root, &source(), 60).expect("a folder");
        let second = Bundle::create(&root, &source(), 60).expect("a second folder");
        assert_ne!(first.folder, second.folder);
        assert!(first.folder.is_dir() && second.folder.is_dir());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_manifest_is_written_and_read_back() {
        let root = scratch("manifest");
        let mut bundle = Bundle::create(&root, &source(), 30).expect("a folder");
        bundle.manifest.duration_ms = 4200.0;
        bundle.manifest.system_audio = Some(SYSTEM_AUDIO.to_owned());
        bundle.manifest.pointer_samples = 12;
        bundle.write_manifest().expect("written");

        let read = Bundle::open(&bundle.folder).expect("a bundle");
        assert_eq!(read.manifest, bundle.manifest);
        assert_eq!(read.video(), bundle.folder.join(VIDEO));
        assert_eq!(read.system_audio(), Some(bundle.folder.join(SYSTEM_AUDIO)));
        assert_eq!(read.microphone(), None);
        assert_eq!(read.pointer(), bundle.folder.join("screen.mp4.cursor.json"));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_folder_without_a_manifest_is_not_a_bundle() {
        let root = scratch("bare");
        std::fs::create_dir_all(root.join("not a take")).expect("a folder");
        assert!(Bundle::open(&root.join("not a take")).is_none());
        assert!(library(&root).is_empty());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn the_library_is_newest_first_and_skips_what_is_not_a_take() {
        let root = scratch("library");
        let older = Bundle::create(&root, &source(), 60).expect("a folder");
        let mut older = older;
        older.manifest.began_at = 1000.0;
        older.write_manifest().expect("written");
        let mut newer = Bundle::create(&root, &source(), 60).expect("a folder");
        newer.manifest.began_at = 2000.0;
        newer.write_manifest().expect("written");
        std::fs::write(root.join("stray.txt"), "nothing").expect("a stray file");
        std::fs::create_dir_all(root.join("stray folder")).expect("a stray folder");

        let found = library(&root);
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].manifest.began_at, 2000.0);
        assert_eq!(found[1].manifest.began_at, 1000.0);
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn an_empty_picture_is_not_a_picture() {
        let root = scratch("empty");
        let bundle = Bundle::create(&root, &source(), 60).expect("a folder");
        assert!(!bundle.has_picture());
        std::fs::write(bundle.video(), []).expect("an empty file");
        assert!(!bundle.has_picture());
        std::fs::write(bundle.video(), [0u8; 8]).expect("a file with something in it");
        assert!(bundle.has_picture());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_discarded_take_leaves_nothing_and_can_be_discarded_twice() {
        let root = scratch("discard");
        let bundle = Bundle::create(&root, &source(), 60).expect("a folder");
        std::fs::write(bundle.video(), [0u8; 8]).expect("a file");
        bundle.discard().expect("discarded");
        assert!(!bundle.folder.exists());
        bundle.discard().expect("discarding again is not an error");
        std::fs::remove_dir_all(&root).ok();
    }
}
