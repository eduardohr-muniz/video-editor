// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Jareer Abdullah and Concat contributors

//! The camera over a clip: a zoom that moves in, holds, and moves back out.
//!
//! A screen recording is unwatchable at full size - a 2880-pixel desktop in
//! a 1080-pixel frame is a tutorial nobody can read - and cutting it up does
//! not fix that. What fixes it is a camera: the frame pushes in on whatever
//! is being clicked, holds while it happens, and pulls back out. Doing it by
//! hand means keying scale and both offsets three times per moment, which is
//! why the editors people buy for this do it for them.
//!
//! A [`Camera`] is a list of [`Move`]s in clip-local time, each one a depth,
//! a point to push in on and how long the push takes. [`Camera::apply`]
//! turns one into a [`Transform`] at an instant, *over* whatever the clip
//! and its keys already say - so a camera move composes with a pan the
//! person keyed by hand instead of fighting it.
//!
//! Two decisions worth knowing about:
//!
//! - **The focus point is in frame fractions, not source fractions.** A
//!   camera zooms the frame about a point of it, which is exact whatever the
//!   clip's size, aspect, crop or rotation; a point of the *source* would
//!   need the fitted size, which is only known further down the compositor.
//!   For a screen recording filling a frame of its own shape - the case this
//!   is for - the two are the same number.
//! - **A move owns a path, not just a point.** A camera that holds the
//!   pointer in frame needs the focus to travel, and the smoothing that
//!   makes it watchable belongs where the pointer's path is read, not in the
//!   compositor. So the editor may bake a focus path into the move and the
//!   engine only ever interpolates it.
//!
//! At depth one the transform is exactly the one it was handed, whatever the
//! focus - which is what makes the ramps at the ends of a move seamless.

use crate::animate::Ease;
use crate::time::Rational;
use crate::timeline::Transform;

/// The shallowest a move can be: no zoom at all.
pub const MIN_DEPTH: f64 = 1.0;

/// The deepest a move can be. Past about eight the picture is pixels, and a
/// number that cannot be reached is better than one that produces mush.
pub const MAX_DEPTH: f64 = 8.0;

/// A point of the frame, as fractions of its width and height.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Focus {
    /// Across, `0..=1`.
    pub x: f64,
    /// Down, `0..=1`.
    pub y: f64,
}

impl Focus {
    /// The middle of the frame: the focus of a move that pushes straight in.
    pub const CENTRE: Focus = Focus { x: 0.5, y: 0.5 };

    /// A point, held inside the frame. Anything that is not a number is the
    /// centre, which is the one answer that cannot send a frame somewhere
    /// nothing was recorded.
    pub fn new(x: f64, y: f64) -> Focus {
        if !x.is_finite() || !y.is_finite() {
            return Focus::CENTRE;
        }
        Focus {
            x: x.clamp(0.0, 1.0),
            y: y.clamp(0.0, 1.0),
        }
    }

    /// This point a fraction of the way to another.
    pub fn towards(self, other: Focus, fraction: f64) -> Focus {
        let fraction = fraction.clamp(0.0, 1.0);
        Focus {
            x: self.x + (other.x - self.x) * fraction,
            y: self.y + (other.y - self.y) * fraction,
        }
    }
}

/// A point the focus passes through, in clip-local time.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Waypoint {
    /// When, in clip-local seconds.
    pub at: Rational,
    /// Where the camera is looking then.
    pub focus: Focus,
}

/// One push in, hold and pull out.
#[derive(Clone, PartialEq, Debug)]
pub struct Move {
    /// When the push begins, in clip-local seconds.
    pub start: Rational,
    /// When the pull-out has finished.
    pub end: Rational,
    /// How far in, as a multiple of the frame. One is no zoom.
    pub depth: f64,
    /// What to push in on, when the move has no path.
    pub focus: Focus,
    /// How long the push in takes.
    pub ease_in: Rational,
    /// How long the pull out takes.
    pub ease_out: Rational,
    /// The curve both ramps follow.
    pub ease: Ease,
    /// Where the camera looks over the move, when it travels. Empty for a
    /// move that holds one point. Times are clip-local, as `start` is.
    pub path: Vec<Waypoint>,
}

impl Move {
    /// How long a push takes when nothing says otherwise: two fifths of a
    /// second. Fast enough to feel deliberate, slow enough not to lurch.
    pub fn ease() -> Rational {
        Rational::new(2, 5)
    }

    /// A move onto one point, with the default ramps at both ends.
    pub fn onto(start: Rational, end: Rational, depth: f64, focus: Focus) -> Move {
        Move {
            start,
            end,
            depth,
            focus,
            ease_in: Move::ease(),
            ease_out: Move::ease(),
            ease: Ease::IN_OUT,
            path: Vec::new(),
        }
    }

    /// How long the move runs for.
    pub fn duration(&self) -> Rational {
        (self.end - self.start).max(Rational::ZERO)
    }

    /// Whether this move does anything at all.
    pub fn is_usable(&self) -> bool {
        self.duration() > Rational::ZERO && self.depth > MIN_DEPTH
    }

    /// The two ramps, shortened to fit when together they are longer than
    /// the move: a half-second push and a half-second pull in a
    /// half-second move become a quarter each, so the shape is kept and the
    /// move never ramps past its own end.
    fn ramps(&self) -> (Rational, Rational) {
        let span = self.duration();
        let (into, out) = (
            self.ease_in.max(Rational::ZERO),
            self.ease_out.max(Rational::ZERO),
        );
        let total = into + out;
        if total <= span || total.is_zero() {
            return (into, out);
        }
        let share = span / total;
        (into * share, out * share)
    }

    /// How far into the move the camera is at `local`, `0..=1`: zero at
    /// either end, one through the hold. None when `local` is outside the
    /// move.
    pub fn progress_at(&self, local: Rational) -> Option<f64> {
        if local < self.start || local > self.end || !self.is_usable() {
            return None;
        }
        let (into, out) = self.ramps();
        let since = local - self.start;
        let until = self.end - local;
        let raw = if !into.is_zero() && since < into {
            (since / into).as_f64()
        } else if !out.is_zero() && until < out {
            (until / out).as_f64()
        } else {
            1.0
        };
        Some(self.ease.apply(raw.clamp(0.0, 1.0)))
    }

    /// Where the camera is looking at `local`: the point it holds, or the
    /// place its path has reached.
    pub fn focus_at(&self, local: Rational) -> Focus {
        if self.path.is_empty() {
            return self.focus;
        }
        let first = self.path[0];
        if local <= first.at {
            return first.focus;
        }
        let last = self.path[self.path.len() - 1];
        if local >= last.at {
            return last.focus;
        }
        // Linear between the two waypoints that straddle `local`. The path
        // is already smoothed where it was made; smoothing it again here
        // would be guessing at the same curve twice.
        let crossed = self
            .path
            .windows(2)
            .find(|pair| local >= pair[0].at && local <= pair[1].at);
        match crossed {
            Some(pair) => {
                let span = pair[1].at - pair[0].at;
                let fraction = if span.is_zero() {
                    0.0
                } else {
                    ((local - pair[0].at) / span).as_f64()
                };
                pair[0].focus.towards(pair[1].focus, fraction)
            }
            None => self.focus,
        }
    }

    /// The move held to what can be drawn: a finite span, a depth in range,
    /// a focus inside the frame, and a path in time order. What every
    /// reader of a document and every command makes of a move before it is
    /// kept.
    pub fn tidy(mut self) -> Move {
        if !self.start.as_f64().is_finite() {
            self.start = Rational::ZERO;
        }
        if self.end < self.start {
            self.end = self.start;
        }
        self.depth = if self.depth.is_finite() {
            self.depth.clamp(MIN_DEPTH, MAX_DEPTH)
        } else {
            MIN_DEPTH
        };
        self.focus = Focus::new(self.focus.x, self.focus.y);
        self.ease_in = self.ease_in.max(Rational::ZERO);
        self.ease_out = self.ease_out.max(Rational::ZERO);
        self.path.retain(|point| point.at >= Rational::ZERO);
        for point in &mut self.path {
            point.focus = Focus::new(point.focus.x, point.focus.y);
        }
        self.path.sort_by(|left, right| left.at.cmp(&right.at));
        self
    }
}

/// Every move over one clip.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct Camera {
    /// In time order, and not overlapping: where two moves would overlap,
    /// the one that starts first owns the instant.
    pub moves: Vec<Move>,
}

/// What the camera is doing at one instant.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Framing {
    /// How far in, as a multiple of the frame.
    pub zoom: f64,
    /// What the frame is centred on.
    pub focus: Focus,
}

impl Framing {
    /// Not zoomed at all.
    pub const NONE: Framing = Framing {
        zoom: 1.0,
        focus: Focus::CENTRE,
    };

    /// Whether this would change the picture.
    pub fn is_none(&self) -> bool {
        (self.zoom - 1.0).abs() < 1e-9
    }
}

impl Camera {
    /// A camera with these moves, tidied, in order, and with overlaps cut
    /// back so that no instant is claimed twice. A move left with nothing
    /// to do is dropped.
    pub fn new(moves: impl IntoIterator<Item = Move>) -> Camera {
        let mut moves: Vec<Move> = moves.into_iter().map(Move::tidy).collect();
        moves.sort_by(|left, right| left.start.cmp(&right.start));
        let mut kept: Vec<Move> = Vec::with_capacity(moves.len());
        for mut next in moves {
            if let Some(last) = kept.last().filter(|last| next.start < last.end) {
                next.start = last.end;
                if next.end < next.start {
                    next.end = next.start;
                }
            }
            if next.is_usable() {
                kept.push(next);
            }
        }
        Camera { moves: kept }
    }

    /// Whether the camera ever does anything.
    pub fn is_empty(&self) -> bool {
        self.moves.is_empty()
    }

    /// What the camera is doing at `local`, in clip-local seconds.
    pub fn at(&self, local: Rational) -> Framing {
        let Some(active) = self
            .moves
            .iter()
            .find(|one| local >= one.start && local <= one.end)
        else {
            return Framing::NONE;
        };
        let Some(progress) = active.progress_at(local) else {
            return Framing::NONE;
        };
        Framing {
            zoom: 1.0 + (active.depth - 1.0) * progress,
            focus: active.focus_at(local),
        }
    }

    /// `base`, zoomed as the camera says at `local`.
    ///
    /// The frame is scaled about the focus point: the point stays where it
    /// is and everything else moves away from it. The picture's centre, at
    /// `0.5 + offset` in frame fractions, moves with everything else, which
    /// is the whole of the arithmetic below.
    pub fn apply(&self, base: Transform, local: Rational) -> Transform {
        Camera::framed(base, self.at(local))
    }

    /// `base` under one resolved framing. Separate from [`Camera::apply`] so
    /// a caller that has already resolved the framing - a preview scrubbing
    /// the same instant for several layers - does not resolve it again.
    pub fn framed(base: Transform, framing: Framing) -> Transform {
        if framing.is_none() {
            return base;
        }
        let zoom = framing.zoom;
        let centre_x = 0.5 + base.offset_x;
        let centre_y = 0.5 + base.offset_y;
        Transform {
            scale: base.scale * zoom,
            offset_x: framing.focus.x + (centre_x - framing.focus.x) * zoom - 0.5,
            offset_y: framing.focus.y + (centre_y - framing.focus.y) * zoom - 0.5,
            ..base
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seconds(value: i64) -> Rational {
        Rational::from_int(value)
    }

    fn at(numerator: i64, denominator: i64) -> Rational {
        Rational::new(numerator, denominator)
    }

    #[test]
    fn a_focus_is_held_inside_the_frame() {
        assert_eq!(Focus::new(1.4, -0.2), Focus { x: 1.0, y: 0.0 });
        assert_eq!(Focus::new(f64::NAN, 0.3), Focus::CENTRE);
        assert_eq!(Focus::new(0.25, 0.75), Focus { x: 0.25, y: 0.75 });
    }

    #[test]
    fn depth_one_leaves_the_transform_exactly_as_it_was() {
        let base = Transform {
            scale: 1.3,
            offset_x: 0.2,
            offset_y: -0.1,
            rotation: 12.0,
            stretch_x: 1.0,
            stretch_y: 1.0,
        };
        let camera = Camera::new([Move::onto(
            seconds(0),
            seconds(2),
            1.0,
            Focus::new(0.1, 0.9),
        )]);
        assert!(camera.is_empty(), "a move that does nothing is not kept");
        assert_eq!(camera.apply(base, seconds(1)), base);
        assert_eq!(Camera::framed(base, Framing::NONE), base);
    }

    #[test]
    fn the_focus_point_stays_where_it_is_while_the_frame_pushes_in() {
        // The picture fitted and centred, zoomed twice about a point a
        // quarter across. That point is 0.25 from the centre before the
        // zoom and must still be at 0.25 of the frame after it.
        let base = Transform::IDENTITY;
        let focus = Focus::new(0.25, 0.5);
        let framed = Camera::framed(base, Framing { zoom: 2.0, focus });
        assert_eq!(framed.scale, 2.0);
        // Where does the frame point that was at `focus` end up? The
        // picture's centre moved to 0.5 + offset_x; the focus was
        // (focus - centre) away from it and is now twice that.
        let centre_after = 0.5 + framed.offset_x;
        let focus_after = centre_after + (focus.x - 0.5) * 2.0;
        assert!(
            (focus_after - focus.x).abs() < 1e-12,
            "the focus moved to {focus_after}"
        );
    }

    #[test]
    fn a_zoom_about_the_centre_only_scales() {
        let framed = Camera::framed(
            Transform::IDENTITY,
            Framing {
                zoom: 3.0,
                focus: Focus::CENTRE,
            },
        );
        assert_eq!(framed.scale, 3.0);
        assert_eq!(framed.offset_x, 0.0);
        assert_eq!(framed.offset_y, 0.0);
    }

    #[test]
    fn a_camera_composes_with_a_pan_the_clip_already_has() {
        // A clip already pushed off to one side stays pushed off, and the
        // camera's zoom is on top of its scale rather than instead of it.
        let base = Transform {
            scale: 1.5,
            offset_x: 0.1,
            ..Transform::IDENTITY
        };
        let framed = Camera::framed(
            base,
            Framing {
                zoom: 2.0,
                focus: Focus::CENTRE,
            },
        );
        assert_eq!(framed.scale, 3.0);
        assert!((framed.offset_x - 0.2).abs() < 1e-12);
    }

    #[test]
    fn a_move_ramps_in_holds_and_ramps_out() {
        let one = Move {
            ease: Ease::LINEAR,
            ..Move::onto(seconds(1), seconds(5), 2.0, Focus::CENTRE)
        };
        let camera = Camera::new([one]);
        // Before and after: nothing.
        assert!(camera.at(seconds(0)).is_none());
        assert!(camera.at(seconds(6)).is_none());
        // At each end of the move itself: still nothing, which is what
        // makes the ends seamless.
        assert!(camera.at(seconds(1)).is_none());
        assert!(camera.at(seconds(5)).is_none());
        // Half way up the push in (the ramp is 2/5 of a second).
        let rising = camera.at(at(6, 5)).zoom;
        assert!((rising - 1.5).abs() < 1e-9, "{rising}");
        // Through the hold.
        assert_eq!(camera.at(seconds(3)).zoom, 2.0);
        // Half way down the pull out.
        let falling = camera.at(at(24, 5)).zoom;
        assert!((falling - 1.5).abs() < 1e-9, "{falling}");
    }

    #[test]
    fn ramps_longer_than_the_move_are_shortened_to_fit() {
        let one = Move {
            ease_in: seconds(1),
            ease_out: seconds(1),
            ease: Ease::LINEAR,
            ..Move::onto(seconds(0), seconds(1), 2.0, Focus::CENTRE)
        };
        // A second of push and a second of pull in a one-second move: half
        // each, so the move reaches its full depth exactly in the middle
        // and nowhere else.
        assert_eq!(one.ramps(), (at(1, 2), at(1, 2)));
        let camera = Camera::new([one]);
        assert_eq!(camera.at(at(1, 2)).zoom, 2.0);
        assert!((camera.at(at(1, 4)).zoom - 1.5).abs() < 1e-9);
    }

    #[test]
    fn a_move_with_no_ramps_is_a_hard_cut_to_the_zoom() {
        let one = Move {
            ease_in: Rational::ZERO,
            ease_out: Rational::ZERO,
            ..Move::onto(seconds(0), seconds(2), 2.0, Focus::CENTRE)
        };
        let camera = Camera::new([one]);
        assert_eq!(camera.at(seconds(0)).zoom, 2.0);
        assert_eq!(camera.at(seconds(1)).zoom, 2.0);
        assert_eq!(camera.at(seconds(2)).zoom, 2.0);
    }

    #[test]
    fn the_focus_travels_along_a_path() {
        let one = Move {
            path: vec![
                Waypoint {
                    at: seconds(0),
                    focus: Focus::new(0.25, 0.5),
                },
                Waypoint {
                    at: seconds(2),
                    focus: Focus::new(0.75, 0.5),
                },
            ],
            ..Move::onto(seconds(0), seconds(2), 2.0, Focus::CENTRE)
        };
        assert_eq!(one.focus_at(seconds(0)), Focus::new(0.25, 0.5));
        assert_eq!(one.focus_at(seconds(1)), Focus::new(0.5, 0.5));
        assert_eq!(one.focus_at(seconds(2)), Focus::new(0.75, 0.5));
        // Outside the path's own span it holds its ends.
        assert_eq!(one.focus_at(seconds(-1)), Focus::new(0.25, 0.5));
        assert_eq!(one.focus_at(seconds(9)), Focus::new(0.75, 0.5));
    }

    #[test]
    fn overlapping_moves_are_cut_back_so_no_instant_is_claimed_twice() {
        let camera = Camera::new([
            Move::onto(seconds(0), seconds(4), 2.0, Focus::CENTRE),
            Move::onto(seconds(2), seconds(6), 3.0, Focus::CENTRE),
        ]);
        assert_eq!(camera.moves.len(), 2);
        assert_eq!(camera.moves[0].end, seconds(4));
        assert_eq!(camera.moves[1].start, seconds(4));
        assert_eq!(camera.moves[1].end, seconds(6));
    }

    #[test]
    fn a_move_swallowed_whole_by_an_earlier_one_is_dropped() {
        let camera = Camera::new([
            Move::onto(seconds(0), seconds(10), 2.0, Focus::CENTRE),
            Move::onto(seconds(2), seconds(3), 3.0, Focus::CENTRE),
        ]);
        assert_eq!(camera.moves.len(), 1);
        assert_eq!(camera.moves[0].end, seconds(10));
    }

    #[test]
    fn moves_are_sorted_however_they_arrive() {
        let camera = Camera::new([
            Move::onto(seconds(8), seconds(9), 2.0, Focus::CENTRE),
            Move::onto(seconds(1), seconds(2), 2.0, Focus::CENTRE),
        ]);
        assert_eq!(camera.moves[0].start, seconds(1));
        assert_eq!(camera.moves[1].start, seconds(8));
    }

    #[test]
    fn nonsense_in_a_move_is_tidied_rather_than_trusted() {
        let one = Move {
            depth: f64::INFINITY,
            focus: Focus { x: 9.0, y: -9.0 },
            ease_in: seconds(-3),
            end: seconds(-1),
            path: vec![Waypoint {
                at: seconds(-5),
                focus: Focus { x: 2.0, y: 2.0 },
            }],
            ..Move::onto(seconds(0), seconds(2), 2.0, Focus::CENTRE)
        }
        .tidy();
        assert_eq!(one.depth, MIN_DEPTH);
        assert_eq!(one.end, one.start);
        assert_eq!(one.ease_in, Rational::ZERO);
        assert_eq!(one.focus, Focus { x: 1.0, y: 0.0 });
        assert!(one.path.is_empty(), "a waypoint before the clip is dropped");
        assert!(!one.is_usable());
    }

    #[test]
    fn a_zoom_about_a_point_near_an_edge_still_covers_the_frame() {
        // The picture only ever grows about the focus, so a move onto
        // something in the corner of the screen cannot show black. This is
        // why there is no "deepest usable zoom" for a focus: there isn't one.
        let framed = Camera::framed(
            Transform::IDENTITY,
            Framing {
                zoom: 2.0,
                focus: Focus::new(0.02, 0.98),
            },
        );
        // Where the picture's edges land, in frame fractions.
        let centre_x = 0.5 + framed.offset_x;
        let (left, right) = (centre_x - framed.scale / 2.0, centre_x + framed.scale / 2.0);
        assert!(left <= 0.0 && right >= 1.0, "{left}..{right}");
        let centre_y = 0.5 + framed.offset_y;
        let (top, bottom) = (centre_y - framed.scale / 2.0, centre_y + framed.scale / 2.0);
        assert!(top <= 0.0 && bottom >= 1.0, "{top}..{bottom}");
    }

    #[test]
    fn an_empty_camera_changes_nothing() {
        let camera = Camera::default();
        assert!(camera.is_empty());
        assert!(camera.at(seconds(3)).is_none());
        assert_eq!(
            camera.apply(Transform::IDENTITY, seconds(3)),
            Transform::IDENTITY
        );
    }
}
