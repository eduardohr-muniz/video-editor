// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Jareer Abdullah and Concat contributors

//! The camera as a document holds it.
//!
//! A clip carries a list of [`CameraMove`]s: when the frame pushes in, how
//! far, onto what, and how long each ramp takes. They live on the clip and
//! in clip-local seconds, which is the whole reason this is not a lane of
//! its own: split the clip and each half keeps the moves over its own
//! picture; ripple the timeline and they travel with it; trim the clip and
//! [`rewindow`](CameraMove::rewindow) moves them with the window. None of
//! that needed new code in the cutting commands, which is exactly what a
//! model earns its keep by not needing.
//!
//! [`engine`] turns them into a [`concat_core::camera::Camera`], which the
//! frame plan applies over the clip's placement and keys. Preview and export
//! both go through it, so what is scrubbed is what is rendered.
//!
//! Depth is a multiple of the frame: 1 is no zoom, 2 is twice as close. The
//! focus is a point of the *frame*, `0..=1` across and down - see
//! `concat_core::camera` for why the frame and not the source.

use serde::{Deserialize, Serialize};

use crate::model::KeyEase;

/// How a move came to be, which is only ever shown and never acted on: an
/// automatic move is drawn differently in the timeline so a person can see
/// at a glance what the editor suggested and what they placed themselves.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CameraMode {
    /// Placed by hand.
    #[default]
    Manual,
    /// Suggested from the pointer's path. Editing one does not make it
    /// manual: a suggestion that was nudged is still the editor's idea.
    Auto,
}

/// A point the camera's focus passes through, in clip-local seconds.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CameraWaypoint {
    /// When, in clip-local seconds.
    pub at: f64,
    /// Across the frame, `0..=1`.
    pub x: f64,
    /// Down the frame, `0..=1`.
    pub y: f64,
}

/// One push in, hold and pull out.
#[cfg_attr(feature = "schema", derive(schemars::JsonSchema))]
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CameraMove {
    /// Minted by the editor ("cam4"); never shared between clips.
    pub id: String,
    /// When the push begins, in clip-local seconds.
    pub start: f64,
    /// When the pull-out has finished, in clip-local seconds.
    pub end: f64,
    /// How far in, as a multiple of the frame. One is no zoom.
    pub depth: f64,
    /// What to push in on, across the frame.
    pub focus_x: f64,
    /// And down it.
    pub focus_y: f64,
    /// How long the push in takes, in seconds.
    pub ease_in: f64,
    /// How long the pull out takes, in seconds.
    pub ease_out: f64,
    /// The curve both ramps follow.
    #[serde(default = "CameraMove::default_ease")]
    pub ease: KeyEase,
    /// Whether this was suggested or placed.
    #[serde(default, skip_serializing_if = "is_manual")]
    pub mode: CameraMode,
    /// Where the focus travels over the move, when it travels. Empty for a
    /// move that holds one point.
    ///
    /// Baked here rather than resolved at render time on purpose: the
    /// smoothing that makes a pointer-following camera watchable belongs
    /// where the pointer's path is read, and a document that carries the
    /// result renders the same on a machine that no longer has the
    /// recording's telemetry beside it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub path: Vec<CameraWaypoint>,
}

/// For `skip_serializing_if`: a move placed by hand says nothing.
fn is_manual(mode: &CameraMode) -> bool {
    *mode == CameraMode::Manual
}

impl CameraMove {
    /// How long a ramp takes when nothing says otherwise.
    pub const EASE: f64 = 0.4;

    /// The depth a move is given when one is placed with no number.
    pub const DEPTH: f64 = 2.0;

    /// The shallowest and deepest a move can be; the engine's own limits.
    pub const MIN_DEPTH: f64 = concat_core::camera::MIN_DEPTH;
    /// See [`CameraMove::MIN_DEPTH`].
    pub const MAX_DEPTH: f64 = concat_core::camera::MAX_DEPTH;

    /// The curve a move follows when a document does not say: slow at both
    /// ends, which is what a camera move looks like when a person does it.
    pub fn default_ease() -> KeyEase {
        KeyEase::IN_OUT
    }

    /// A move onto one point of the frame, with the usual ramps.
    pub fn onto(
        id: impl Into<String>,
        start: f64,
        end: f64,
        depth: f64,
        focus: (f64, f64),
    ) -> CameraMove {
        CameraMove {
            id: id.into(),
            start,
            end,
            depth,
            focus_x: focus.0,
            focus_y: focus.1,
            ease_in: CameraMove::EASE,
            ease_out: CameraMove::EASE,
            ease: CameraMove::default_ease(),
            mode: CameraMode::Manual,
            path: Vec::new(),
        }
    }

    /// How long the move runs for, in seconds.
    pub fn duration(&self) -> f64 {
        (self.end - self.start).max(0.0)
    }

    /// Whether the move would do anything.
    pub fn is_usable(&self) -> bool {
        self.duration() > 0.0 && self.depth > CameraMove::MIN_DEPTH
    }

    /// The move held to what can be drawn: finite times in order, a depth
    /// in range, a focus inside the frame, ramps that are not negative, and
    /// a path in time order. What the document reader and every command
    /// make of a move before it is kept.
    pub fn tidy(mut self) -> CameraMove {
        self.start = if self.start.is_finite() {
            self.start.max(0.0)
        } else {
            0.0
        };
        self.end = if self.end.is_finite() {
            self.end.max(self.start)
        } else {
            self.start
        };
        self.depth = if self.depth.is_finite() {
            self.depth
                .clamp(CameraMove::MIN_DEPTH, CameraMove::MAX_DEPTH)
        } else {
            CameraMove::MIN_DEPTH
        };
        self.focus_x = fraction(self.focus_x);
        self.focus_y = fraction(self.focus_y);
        self.ease_in = seconds(self.ease_in);
        self.ease_out = seconds(self.ease_out);
        self.path.retain(|point| point.at.is_finite());
        for point in &mut self.path {
            point.x = fraction(point.x);
            point.y = fraction(point.y);
            point.at = point.at.max(0.0);
        }
        self.path
            .sort_by(|left, right| left.at.total_cmp(&right.at));
        self
    }

    /// The move as it falls on a clip that has been retrimmed from `old`
    /// seconds long to the window `from..to` of its old self.
    ///
    /// Returns none for a move the new window does not reach. A move that
    /// straddles an edge is cut at it, which is the only answer that keeps
    /// the picture doing what it was doing at the instant the window begins.
    pub fn rewindow(mut self, from: f64, to: f64) -> Option<CameraMove> {
        if !(from.is_finite() && to.is_finite()) || to <= from {
            return None;
        }
        if self.end <= from || self.start >= to {
            return None;
        }
        let shift = |time: f64| (time - from).clamp(0.0, to - from);
        for point in &mut self.path {
            point.at = shift(point.at);
        }
        self.start = shift(self.start);
        self.end = shift(self.end);
        self.is_usable().then_some(self)
    }

    /// The move's focus at a clip-local instant, following its path when it
    /// has one. What the UI draws a move's marker at.
    pub fn focus_at(&self, local: f64) -> (f64, f64) {
        if self.path.is_empty() {
            return (self.focus_x, self.focus_y);
        }
        let first = self.path[0];
        if local <= first.at {
            return (first.x, first.y);
        }
        let last = self.path[self.path.len() - 1];
        if local >= last.at {
            return (last.x, last.y);
        }
        match self
            .path
            .windows(2)
            .find(|pair| local >= pair[0].at && local <= pair[1].at)
        {
            Some(pair) => {
                let span = pair[1].at - pair[0].at;
                let fraction = if span > 0.0 {
                    (local - pair[0].at) / span
                } else {
                    0.0
                };
                (
                    pair[0].x + (pair[1].x - pair[0].x) * fraction,
                    pair[0].y + (pair[1].y - pair[0].y) * fraction,
                )
            }
            None => (self.focus_x, self.focus_y),
        }
    }
}

/// A fraction of the frame, held into it; anything that is not a number is
/// the middle.
fn fraction(value: f64) -> f64 {
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        0.5
    }
}

/// A length in seconds, never negative.
fn seconds(value: f64) -> f64 {
    if value.is_finite() {
        value.max(0.0)
    } else {
        0.0
    }
}

/// The engine's camera for a clip's moves, or none when none of them would
/// do anything. The engine does the overlap-trimming and the ordering, so
/// this is only a change of spelling.
pub fn engine(moves: &[CameraMove]) -> Option<concat_core::camera::Camera> {
    use concat_core::Rational;
    use concat_core::animate::Ease;
    use concat_core::camera::{Camera, Focus, Move, Waypoint};

    /// Seconds as the engine's exact time. A camera move is a human number
    /// of seconds typed into a panel, so an approximation to the nearest
    /// rational is exactly right here.
    fn exact(value: f64) -> Rational {
        Rational::approximate(value).unwrap_or(Rational::ZERO)
    }

    let camera = Camera::new(moves.iter().filter(|one| one.is_usable()).map(|one| {
        let [x1, y1, x2, y2] = one.ease.sane().0;
        Move {
            start: exact(one.start),
            end: exact(one.end),
            depth: one.depth,
            focus: Focus::new(one.focus_x, one.focus_y),
            ease_in: exact(one.ease_in),
            ease_out: exact(one.ease_out),
            ease: Ease::new(x1, y1, x2, y2),
            path: one
                .path
                .iter()
                .map(|point| Waypoint {
                    at: exact(point.at),
                    focus: Focus::new(point.x, point.y),
                })
                .collect(),
        }
    }));
    (!camera.is_empty()).then_some(camera)
}

/// The deepest any of these moves goes. What a decoder is sized by, so a
/// zoom costs no sharpness the file had.
pub fn deepest(moves: &[CameraMove]) -> f64 {
    moves
        .iter()
        .filter(|one| one.is_usable())
        .map(|one| one.depth)
        .fold(1.0, f64::max)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one() -> CameraMove {
        CameraMove::onto("cam1", 1.0, 5.0, 2.0, (0.25, 0.75))
    }

    #[test]
    fn a_move_knows_how_long_it_runs_and_whether_it_does_anything() {
        assert_eq!(one().duration(), 4.0);
        assert!(one().is_usable());
        assert!(
            !CameraMove {
                depth: 1.0,
                ..one()
            }
            .is_usable()
        );
        assert!(!CameraMove { end: 1.0, ..one() }.is_usable());
    }

    #[test]
    fn nonsense_is_tidied_rather_than_trusted() {
        let tidied = CameraMove {
            start: f64::NAN,
            end: -4.0,
            depth: 900.0,
            focus_x: 3.0,
            focus_y: f64::NEG_INFINITY,
            ease_in: -1.0,
            ease_out: f64::NAN,
            path: vec![
                CameraWaypoint {
                    at: 2.0,
                    x: -1.0,
                    y: 2.0,
                },
                CameraWaypoint {
                    at: 1.0,
                    x: 0.5,
                    y: 0.5,
                },
                CameraWaypoint {
                    at: f64::NAN,
                    x: 0.5,
                    y: 0.5,
                },
            ],
            ..one()
        }
        .tidy();
        assert_eq!(tidied.start, 0.0);
        assert_eq!(tidied.end, 0.0);
        assert_eq!(tidied.depth, CameraMove::MAX_DEPTH);
        assert_eq!(tidied.focus_x, 1.0);
        assert_eq!(tidied.focus_y, 0.5);
        assert_eq!(tidied.ease_in, 0.0);
        assert_eq!(tidied.ease_out, 0.0);
        assert_eq!(tidied.path.len(), 2, "the waypoint with no time is dropped");
        assert_eq!(tidied.path[0].at, 1.0, "the path is put in order");
        assert_eq!(tidied.path[1].x, 0.0);
    }

    #[test]
    fn a_move_outside_a_new_window_is_dropped() {
        assert!(one().rewindow(6.0, 10.0).is_none());
        assert!(one().rewindow(0.0, 1.0).is_none());
        assert!(one().rewindow(3.0, 3.0).is_none());
    }

    #[test]
    fn a_move_inside_a_new_window_moves_with_it() {
        let moved = one().rewindow(1.0, 6.0).expect("the move survives");
        assert_eq!((moved.start, moved.end), (0.0, 4.0));
        assert_eq!(moved.depth, 2.0);
    }

    #[test]
    fn a_move_straddling_the_new_in_point_is_cut_at_it() {
        // The window starts in the middle of the move: what is left begins
        // at zero and is as long as the part that survived.
        let moved = one().rewindow(3.0, 9.0).expect("the move survives");
        assert_eq!((moved.start, moved.end), (0.0, 2.0));
    }

    #[test]
    fn a_move_straddling_the_new_out_point_is_cut_at_it() {
        let moved = one().rewindow(0.0, 3.0).expect("the move survives");
        assert_eq!((moved.start, moved.end), (1.0, 3.0));
    }

    #[test]
    fn a_path_moves_with_the_window_too() {
        let mut move_with_path = one();
        move_with_path.path = vec![
            CameraWaypoint {
                at: 1.0,
                x: 0.2,
                y: 0.2,
            },
            CameraWaypoint {
                at: 5.0,
                x: 0.8,
                y: 0.8,
            },
        ];
        let moved = move_with_path.rewindow(1.0, 6.0).expect("it survives");
        assert_eq!(moved.path[0].at, 0.0);
        assert_eq!(moved.path[1].at, 4.0);
    }

    #[test]
    fn the_focus_follows_a_path_when_there_is_one() {
        let mut travelling = one();
        assert_eq!(travelling.focus_at(2.0), (0.25, 0.75));
        travelling.path = vec![
            CameraWaypoint {
                at: 1.0,
                x: 0.0,
                y: 0.0,
            },
            CameraWaypoint {
                at: 3.0,
                x: 1.0,
                y: 1.0,
            },
        ];
        assert_eq!(travelling.focus_at(0.0), (0.0, 0.0));
        assert_eq!(travelling.focus_at(2.0), (0.5, 0.5));
        assert_eq!(travelling.focus_at(9.0), (1.0, 1.0));
    }

    #[test]
    fn a_clip_with_no_usable_move_has_no_camera() {
        assert!(engine(&[]).is_none());
        assert!(
            engine(&[CameraMove {
                depth: 1.0,
                ..one()
            }])
            .is_none()
        );
    }

    #[test]
    fn the_engine_s_camera_carries_the_move_across() {
        let camera = engine(&[one()]).expect("a camera");
        assert_eq!(camera.moves.len(), 1);
        let crossed = &camera.moves[0];
        assert_eq!(crossed.depth, 2.0);
        assert_eq!(crossed.focus.x, 0.25);
        assert_eq!(crossed.start.as_f64(), 1.0);
        assert_eq!(crossed.end.as_f64(), 5.0);
        assert!((crossed.ease_in.as_f64() - CameraMove::EASE).abs() < 1e-9);
    }

    #[test]
    fn the_deepest_move_is_what_a_decoder_is_sized_by() {
        assert_eq!(deepest(&[]), 1.0);
        assert_eq!(
            deepest(&[
                one(),
                CameraMove {
                    depth: 3.5,
                    ..one()
                },
                CameraMove {
                    depth: 6.0,
                    end: 1.0,
                    ..one()
                },
            ]),
            3.5,
            "a move that does nothing does not ask for pixels"
        );
    }

    #[test]
    fn a_move_round_trips_through_a_document() {
        let text = serde_json::to_string(&one()).expect("written");
        assert!(
            !text.contains("\"mode\""),
            "a manual move says nothing: {text}"
        );
        assert!(
            !text.contains("\"path\""),
            "an empty path says nothing: {text}"
        );
        let read: CameraMove = serde_json::from_str(&text).expect("read back");
        assert_eq!(read, one());
    }

    #[test]
    fn an_automatic_move_says_so_in_the_document() {
        let suggested = CameraMove {
            mode: CameraMode::Auto,
            ..one()
        };
        let text = serde_json::to_string(&suggested).expect("written");
        assert!(text.contains("\"mode\":\"auto\""), "{text}");
        assert_eq!(
            serde_json::from_str::<CameraMove>(&text).expect("read back"),
            suggested
        );
    }
}
