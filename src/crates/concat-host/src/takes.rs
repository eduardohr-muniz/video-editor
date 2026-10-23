// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Jareer Abdullah and Concat contributors

//! Bringing a recording into a project.
//!
//! A take is a folder: the picture, the desktop's sound and the microphone
//! as separate files, and the pointer's path beside the picture (see
//! `concat_capture::bundle`). This module turns one into clips.
//!
//! The sound stays apart. The picture goes on the first picture lane, the
//! desktop's sound on one sound lane and the microphone on another, all
//! three starting together. Muxing them into one file at import would cost
//! a transcode and take away the one thing anyone wants from a screen
//! recording's sound: the room noise cleaned off the microphone and the
//! desktop left alone. Three media items and three clips cost nothing and
//! keep that.
//!
//! Then the camera. The pointer's path says where the clicks were, so
//! `concat_project::autozoom` suggests the moves and they are added to the
//! picture's clip as one undoable step. An import of a take made without
//! the pointer followed, or of footage recorded somewhere else, simply has
//! none - which is the same code path with an empty answer.
//!
//! The pointer's path is in fractions of what was *captured*; a camera move
//! is in fractions of the *frame*. For a recording in a frame of its own
//! shape those are the same number, and for one letterboxed into a frame of
//! another shape they are not - so [`into_frame`] maps them, and a take
//! dropped into a vertical timeline still zooms onto what was clicked.

use std::path::{Path, PathBuf};

use concat_capture::Bundle;
use concat_project::autozoom::{self, Dot, DotKind};
use concat_project::camera::{CameraMode, CameraMove};
use concat_project::commands::Command;
use concat_project::editor::Editor;

use crate::dirs::AppDirs;

/// Where takes are kept: a folder beside the models and the caches, so a
/// recording survives the project it was made for being thrown away.
pub fn recordings(dirs: &AppDirs) -> PathBuf {
    dirs.library().join("recordings")
}

/// Every take on this machine, newest first.
pub fn library(dirs: &AppDirs) -> Vec<Bundle> {
    concat_capture::bundle::library(&recordings(dirs))
}

/// How a take's camera is set up as it comes in.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Zooming {
    /// Suggest moves from the pointer's path.
    pub automatic: bool,
    /// How far each suggested move pushes in.
    pub depth: f64,
}

impl Default for Zooming {
    fn default() -> Zooming {
        Zooming {
            automatic: true,
            depth: CameraMove::DEPTH,
        }
    }
}

/// What an import did.
#[derive(Clone, PartialEq, Debug)]
pub struct Imported {
    /// The picture's clip.
    pub clip_id: String,
    /// The desktop sound's clip, when the take had any.
    pub system_audio_clip: Option<String>,
    /// The microphone's clip, when the take had any.
    pub microphone_clip: Option<String>,
    /// How many camera moves were suggested.
    pub moves: usize,
}

/// Puts `bundle` on the active timeline at `at` seconds, with its sound on
/// its own lanes and its camera suggested from the pointer's path.
///
/// The picture is placed first and is what the result names: a take whose
/// sound will not import is still a take, and the error for one file is not
/// a reason to refuse the others. A take with no readable picture is.
pub fn import(
    editor: &mut Editor,
    bundle: &Bundle,
    at: f64,
    zooming: &Zooming,
) -> Result<Imported, String> {
    let video = bundle.video();
    let video_path = video.to_string_lossy().into_owned();
    let summary = crate::media::probe(&video_path)?;
    let picture = summary
        .video
        .as_ref()
        .ok_or_else(|| format!("{video_path} has no picture in it"))?;
    let (source_width, source_height) = (picture.width, picture.height);

    let media_id = place_media(editor, &video_path)?;
    let track_id = picture_lane(editor).ok_or("the timeline has no picture lane")?;
    let clip_id = place_clip(editor, &media_id, &track_id, at)?;

    // The sound, each on its own lane, and each one's failure its own.
    let mut sound = Vec::new();
    for file in [bundle.system_audio(), bundle.microphone()] {
        let clip = file.and_then(|path| {
            let path = path.to_string_lossy().into_owned();
            match import_sound(editor, &path, at) {
                Ok(clip) => Some(clip),
                Err(error) => {
                    log::warn!("a take's sound did not import: {error}");
                    None
                }
            }
        });
        sound.push(clip);
    }

    let moves = if zooming.automatic {
        suggest(
            editor,
            bundle,
            &clip_id,
            (source_width, source_height),
            zooming.depth,
        )
    } else {
        0
    };

    Ok(Imported {
        clip_id,
        system_audio_clip: sound[0].clone(),
        microphone_clip: sound[1].clone(),
        moves,
    })
}

/// Suggests the camera for a clip from a take's pointer path, and adds the
/// moves as one undoable step. How many were added; zero for a take with no
/// path, one with nothing clicked in it, or a clip whose camera already
/// covers every moment found.
pub fn suggest(
    editor: &mut Editor,
    bundle: &Bundle,
    clip_id: &str,
    source: (u32, u32),
    depth: f64,
) -> usize {
    let telemetry = concat_capture::Telemetry::read(&bundle.pointer());
    if telemetry.samples.is_empty() {
        return 0;
    }

    let Some(clip) = editor
        .project()
        .active()
        .clips
        .iter()
        .find(|clip| clip.id == clip_id)
    else {
        return 0;
    };
    let length_ms = clip.duration * 1000.0;
    let reserved: Vec<autozoom::Reserved> = clip
        .camera
        .iter()
        .map(|one| autozoom::Reserved {
            start_ms: one.start * 1000.0,
            end_ms: one.end * 1000.0,
        })
        .collect();
    // What the take's own in-point is, so a clip of the middle of a take
    // gets the part of the path that is under it.
    let from_ms = clip.source_start * 1000.0;
    let frame = {
        let video = &editor.project().active().video;
        (video.width, video.height)
    };

    let path: Vec<Dot> = telemetry
        .window(from_ms, from_ms + length_ms)
        .into_iter()
        .map(|sample| {
            let (x, y) = into_frame((sample.cx, sample.cy), source, frame);
            Dot {
                at_ms: sample.time_ms,
                x,
                y,
                kind: kind_of(sample.interaction_type),
            }
        })
        .collect();

    let mut next = 0;
    let suggested = autozoom::suggest(&path, length_ms, &reserved, depth, || {
        next += 1;
        // Named by the editor's own mint when the command is applied; this
        // is only to keep the suggestions apart from each other inside the
        // function, and never reaches the document.
        format!("pending{next}")
    })
    .moves();
    if suggested.is_empty() {
        return 0;
    }

    let count = suggested.len();
    let batch = Command::Batch {
        commands: suggested
            .into_iter()
            .map(|one| Command::AddCameraMove {
                clip_id: clip_id.to_owned(),
                start: one.start,
                end: one.end,
                depth: one.depth,
                focus_x: one.focus_x,
                focus_y: one.focus_y,
                mode: CameraMode::Auto,
            })
            .collect(),
    };
    match editor.apply(batch) {
        Ok(outcome) if outcome.applied => count,
        Ok(_) => 0,
        Err(error) => {
            log::warn!("the suggested camera moves were refused: {error}");
            0
        }
    }
}

/// A point of a `source`-sized picture as a point of a `frame`-sized frame,
/// for a picture fitted and centred in it - which is how an imported take
/// sits until somebody moves it.
///
/// The fitted picture covers a fraction of each axis; the point is that
/// fraction of the way across it from the centre. For a source of the
/// frame's own shape both fractions are one and the point is unchanged.
pub fn into_frame(point: (f64, f64), source: (u32, u32), frame: (u32, u32)) -> (f64, f64) {
    let (sw, sh) = (f64::from(source.0), f64::from(source.1));
    let (fw, fh) = (f64::from(frame.0), f64::from(frame.1));
    if !(sw > 0.0 && sh > 0.0 && fw > 0.0 && fh > 0.0) {
        return point;
    }
    let fit = (fw / sw).min(fh / sh);
    let (across, down) = ((sw * fit / fw).min(1.0), (sh * fit / fh).min(1.0));
    (
        (0.5 + (point.0 - 0.5) * across).clamp(0.0, 1.0),
        (0.5 + (point.1 - 0.5) * down).clamp(0.0, 1.0),
    )
}

/// The pointer path's interaction as the suggester's.
fn kind_of(interaction: Option<concat_capture::Interaction>) -> DotKind {
    use concat_capture::Interaction;
    match interaction {
        Some(Interaction::DoubleClick) => DotKind::PressedTwice,
        Some(Interaction::Click | Interaction::RightClick | Interaction::MiddleClick) => {
            DotKind::Pressed
        }
        Some(Interaction::MouseUp) => DotKind::Released,
        Some(Interaction::Move) | None => DotKind::Moved,
    }
}

/// Adds one file to the bin and gives back its id.
fn place_media(editor: &mut Editor, path: &str) -> Result<String, String> {
    let summary = crate::media::probe(path)?;
    editor
        .apply(Command::AddMedia {
            item: summary.to_new_media(),
        })
        .map_err(|error| error.to_string())?
        .created_id
        .ok_or_else(|| format!("{path} was not added to the bin"))
}

/// Places a clip of `media_id` on `track_id` and gives back its id.
fn place_clip(
    editor: &mut Editor,
    media_id: &str,
    track_id: &str,
    at: f64,
) -> Result<String, String> {
    editor
        .apply(Command::AddClip {
            media_id: media_id.to_owned(),
            track_id: track_id.to_owned(),
            start: at.max(0.0),
            ripple: false,
        })
        .map_err(|error| error.to_string())?
        .created_id
        .ok_or_else(|| "the clip was not placed".to_owned())
}

/// One of a take's sound files, on the first lane with room for it.
fn import_sound(editor: &mut Editor, path: &str, at: f64) -> Result<String, String> {
    let media_id = place_media(editor, path)?;
    editor
        .apply(Command::AddClipAtFirstFree {
            media_id,
            start: at.max(0.0),
        })
        .map_err(|error| error.to_string())?
        .created_id
        .ok_or_else(|| "the sound was not placed".to_owned())
}

/// The lane a take's picture goes on: the lowest lane, which is where a
/// screen recording belongs - the webcam, the titles and everything else in
/// the cut go over it, and it is the lane whose picture the timeline's
/// background sits behind (`Timeline::background_lane`).
///
/// The document's lanes are lowest first: lane zero is drawn first and so
/// is bottom-most in the stack.
fn picture_lane(editor: &Editor) -> Option<String> {
    editor
        .project()
        .active()
        .tracks
        .first()
        .map(|track| track.id.clone())
}

/// Whether a path names a take's folder rather than a plain media file.
pub fn is_take(path: &Path) -> bool {
    path.is_dir() && Bundle::open(path).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_point_of_a_picture_of_the_frame_s_own_shape_is_unchanged() {
        assert_eq!(
            into_frame((0.25, 0.75), (2880, 1800), (1920, 1200)),
            (0.25, 0.75)
        );
        assert_eq!(
            into_frame((0.0, 1.0), (1920, 1080), (1280, 720)),
            (0.0, 1.0)
        );
    }

    #[test]
    fn a_wide_picture_in_a_tall_frame_keeps_its_point_inside_the_letterbox() {
        // A 16:9 take in a 9:16 frame: the picture covers the full width
        // and a thin band of the height, so a point at the top of the take
        // is near the middle of the frame, not at the top of it.
        let (x, y) = into_frame((0.5, 0.0), (1920, 1080), (1080, 1920));
        assert_eq!(x, 0.5);
        // The band is (1080/1920 * 1080) / 1920 of the height.
        let band = (1080.0 * (1080.0 / 1920.0)) / 1920.0;
        assert!((y - (0.5 - band / 2.0)).abs() < 1e-12, "{y}");
        assert!(
            y > 0.3 && y < 0.5,
            "a point at the top of the take is at {y}"
        );
    }

    #[test]
    fn a_tall_picture_in_a_wide_frame_is_the_same_the_other_way_round() {
        let (x, y) = into_frame((0.0, 0.5), (1080, 1920), (1920, 1080));
        assert_eq!(y, 0.5);
        assert!(
            x > 0.3 && x < 0.5,
            "a point at the left of the take is at {x}"
        );
    }

    #[test]
    fn a_size_that_is_not_a_size_leaves_the_point_alone() {
        assert_eq!(
            into_frame((0.25, 0.75), (0, 1080), (1920, 1080)),
            (0.25, 0.75)
        );
        assert_eq!(into_frame((0.25, 0.75), (1920, 1080), (0, 0)), (0.25, 0.75));
    }

    #[test]
    fn the_pointer_s_interactions_become_the_suggester_s() {
        use concat_capture::Interaction;
        assert_eq!(kind_of(Some(Interaction::Click)), DotKind::Pressed);
        assert_eq!(kind_of(Some(Interaction::RightClick)), DotKind::Pressed);
        assert_eq!(
            kind_of(Some(Interaction::DoubleClick)),
            DotKind::PressedTwice
        );
        assert_eq!(kind_of(Some(Interaction::MouseUp)), DotKind::Released);
        assert_eq!(kind_of(Some(Interaction::Move)), DotKind::Moved);
        assert_eq!(kind_of(None), DotKind::Moved);
    }

    #[test]
    fn takes_live_in_their_own_folder_under_the_library() {
        let dirs = AppDirs::under(Path::new("/tmp/concat-test"));
        assert_eq!(
            recordings(&dirs),
            PathBuf::from("/tmp/concat-test/recordings")
        );
        assert!(library(&dirs).is_empty());
    }

    #[test]
    fn a_folder_that_is_not_a_take_is_not_a_take() {
        assert!(!is_take(Path::new("/tmp")));
        assert!(!is_take(Path::new("/tmp/concat-capture-nowhere")));
    }
}
