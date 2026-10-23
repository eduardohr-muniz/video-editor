// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Jareer Abdullah and Concat contributors

//! The camera over a clip: adding a move, changing one, taking one off.
//!
//! One arm per command, exactly as [`super::apply`] routes them here.
//!
//! Every arm ends in [`Clip::sort_camera`], which tidies and orders what is
//! there and drops a move that would do nothing - so a move dragged until
//! it has no length, or a depth typed down to one, leaves rather than
//! sitting in the document doing nothing. A command that changes nothing
//! reports `applied: false` and the editor records no undo step for it,
//! which is what keeps a drag from filling the undo stack.
//!
//! Suggesting moves is not a command. `crate::autozoom` is a pure function
//! over a pointer path, so whoever has the path - the host, which read it
//! off the disk - calls it and then sends the moves as an ordinary
//! [`Command::Batch`] of [`Command::AddCameraMove`]. That keeps the model
//! crate free of the recording's files and makes the suggestion undoable in
//! one step, for nothing.

use super::*;
use crate::camera::CameraMove;

/// Applies one of this module's commands. Any other is a routing error.
pub(super) fn apply(
    project: &mut Project,
    mint: &mut IdMint,
    command: Command,
) -> Result<Outcome, CommandError> {
    match command {
        Command::AddCameraMove {
            clip_id,
            start,
            end,
            depth,
            focus_x,
            focus_y,
            mode,
        } => {
            let id = mint.next("cam");
            let timeline = project.active_mut();
            let Some(clip) = timeline.clip_mut(&clip_id) else {
                return Ok(Outcome::default());
            };
            // A move is held to the clip it is on: one placed past the end -
            // by a gesture on a clip that has since been trimmed, or by a
            // script working from stale numbers - would never be drawn.
            let length = clip.duration;
            let (start, end) = (start.clamp(0.0, length), end.clamp(0.0, length));
            let placed = CameraMove {
                mode,
                ..CameraMove::onto(
                    id.clone(),
                    start.min(end),
                    end.max(start),
                    depth,
                    (focus_x, focus_y),
                )
            }
            .tidy();
            if !placed.is_usable() {
                return Ok(Outcome::default());
            }
            clip.camera.push(placed);
            clip.sort_camera();
            Ok(Outcome {
                created_id: Some(id),
                applied: true,
            })
        }

        Command::UpdateCameraMove {
            clip_id,
            move_id,
            patch,
        } => {
            let timeline = project.active_mut();
            let Some(clip) = timeline.clip_mut(&clip_id) else {
                return Ok(Outcome::default());
            };
            let length = clip.duration;
            let Some(one) = clip.camera.iter_mut().find(|one| one.id == move_id) else {
                return Ok(Outcome::default());
            };
            let before = one.clone();
            if let Some(start) = patch.start {
                one.start = start.clamp(0.0, length);
            }
            if let Some(end) = patch.end {
                one.end = end.clamp(0.0, length);
            }
            if one.end < one.start {
                std::mem::swap(&mut one.start, &mut one.end);
            }
            if let Some(depth) = patch.depth {
                one.depth = depth;
            }
            if let Some(focus_x) = patch.focus_x {
                one.focus_x = focus_x;
            }
            if let Some(focus_y) = patch.focus_y {
                one.focus_y = focus_y;
            }
            if let Some(ease_in) = patch.ease_in {
                one.ease_in = ease_in;
            }
            if let Some(ease_out) = patch.ease_out {
                one.ease_out = ease_out;
            }
            if let Some(ease) = patch.ease {
                one.ease = ease;
            }
            // A focus dragged by hand is still the editor's suggestion; the
            // mode says where a move came from, not who last touched it.
            let changed = *one != before;
            clip.sort_camera();
            Ok(Outcome {
                created_id: None,
                applied: changed,
            })
        }

        Command::RemoveCameraMove { clip_id, move_id } => {
            let timeline = project.active_mut();
            let Some(clip) = timeline.clip_mut(&clip_id) else {
                return Ok(Outcome::default());
            };
            let before = clip.camera.len();
            clip.camera.retain(|one| one.id != move_id);
            Ok(Outcome {
                created_id: None,
                applied: clip.camera.len() != before,
            })
        }

        Command::ClearCameraMoves {
            clip_id,
            suggested_only,
        } => {
            let timeline = project.active_mut();
            let Some(clip) = timeline.clip_mut(&clip_id) else {
                return Ok(Outcome::default());
            };
            let before = clip.camera.len();
            if suggested_only {
                clip.camera
                    .retain(|one| one.mode != crate::camera::CameraMode::Auto);
            } else {
                clip.camera.clear();
            }
            Ok(Outcome {
                created_id: None,
                applied: clip.camera.len() != before,
            })
        }

        _ => unreachable!("commands::apply routes only this module's commands here"),
    }
}
