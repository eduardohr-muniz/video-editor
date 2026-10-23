// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Jareer Abdullah and Concat contributors

//! Where the camera should move, worked out from what was clicked.
//!
//! A screen recording's own pointer path says where the interesting moments
//! are: people click on the thing they are talking about. So the moves can
//! be suggested rather than placed - which is the difference between an
//! editor that makes a tutorial watchable in ten seconds and one that makes
//! it watchable in ten minutes.
//!
//! The rule, in order:
//!
//! 1. Only presses count. A dwell - the pointer sitting still - reads as a
//!    moment far too often: people park the pointer while they talk, and a
//!    frame that pushes in every time they do is seasick.
//! 2. Presses within [`MERGE_GAP_MS`] of each other are one moment. A
//!    dialog that is opened, filled in and dismissed is one push in, not
//!    four.
//! 3. A moment becomes a move from [`PAD_MS`] before its first press to
//!    [`PAD_MS`] after its last, so the frame arrives before the click and
//!    leaves after its result is on screen.
//! 4. The focus is the press in the cluster that looked most deliberate -
//!    a double click, a dropdown, a selection - because that is the one
//!    whose position the viewer needs to be looking at.
//! 5. A move that would land on a span the caller has reserved is dropped
//!    rather than moved. The reserved spans are the moves already there, so
//!    suggesting again never disturbs what somebody has placed by hand.
//!
//! The classification in step 4 is the port of Recordly's own, which is
//! where this algorithm and its constants come from (both projects are
//! AGPL, and the numbers are the result of someone watching a lot of
//! recordings). It reads the trajectory *after* each press: a drag sideways
//! is a selection, a slow slide downwards is a menu, a pointer that stops
//! dead is a text field. None of it changes where a move goes - only which
//! press inside a cluster the frame centres on.

use crate::camera::{CameraMode, CameraMove};

/// What the pointer did at one sample. The input to this module, kept
/// deliberately plain: the recorder's own file format lives in
/// `concat_capture` and is converted on the way in, so the model crate does
/// not depend on the recorder.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Dot {
    /// Milliseconds into the clip.
    pub at_ms: f64,
    /// Across the frame, `0..=1`.
    pub x: f64,
    /// Down the frame, `0..=1`.
    pub y: f64,
    /// What happened here.
    pub kind: DotKind,
}

/// What a [`Dot`] is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DotKind {
    /// The pointer only moved.
    Moved,
    /// A button went down.
    Pressed,
    /// A button went down twice, close together.
    PressedTwice,
    /// A button came up.
    Released,
}

impl DotKind {
    /// Whether this is a button going down - the only thing a suggestion is
    /// built from.
    pub fn is_press(self) -> bool {
        matches!(self, DotKind::Pressed | DotKind::PressedTwice)
    }
}

/// How far apart two presses can be and still be one moment.
pub const MERGE_GAP_MS: f64 = 2500.0;

/// How long before the first press a move begins, and how long after the
/// last one it ends.
pub const PAD_MS: f64 = 500.0;

/// A span of clip-local milliseconds a suggestion must not touch.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Reserved {
    /// Where it begins.
    pub start_ms: f64,
    /// Where it ends.
    pub end_ms: f64,
}

/// Why there are no suggestions, when there are none. Worth telling apart:
/// each one wants a different sentence on screen.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Nothing {
    /// The clip has no length.
    NoClip,
    /// There is no pointer path - a clip of footage that was not recorded
    /// here, or a take made with the pointer not followed.
    NoPath,
    /// There is a path, and nothing was clicked in it.
    NoClicks,
    /// Every moment found was already covered by a move.
    NoRoom,
}

/// What came of asking.
#[derive(Clone, PartialEq, Debug)]
pub enum Suggestions {
    /// These moves, in time order.
    Moves(Vec<CameraMove>),
    /// None, for this reason.
    None(Nothing),
}

impl Suggestions {
    /// The moves, or an empty list.
    pub fn moves(self) -> Vec<CameraMove> {
        match self {
            Suggestions::Moves(moves) => moves,
            Suggestions::None(_) => Vec::new(),
        }
    }
}

/// How deliberate a press looked, which decides which press in a cluster
/// the frame centres on. The numbers are relative and nothing else reads
/// them.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Deliberate {
    /// A press with no recognisable pattern after it.
    Plain = 0,
    /// A press after which the pointer stopped dead: a text field.
    TextField = 1,
    /// A press followed by a slow slide downwards: a menu being read.
    Dropdown = 2,
    /// A press followed by a drag sideways: text being selected.
    Selection = 3,
    /// Two presses in one place.
    Twice = 4,
}

/// One moment found in the path.
#[derive(Clone, Copy, PartialEq, Debug)]
struct Press {
    at_ms: f64,
    x: f64,
    y: f64,
    looked: Deliberate,
}

/// Moves suggested for a clip `length_ms` long from its pointer path.
///
/// `reserved` is the spans already taken - the clip's own moves, usually -
/// and a suggestion overlapping one is dropped. `depth` is how far each
/// suggested move pushes in; `mint` names them, and is called once per move
/// in the order they are returned.
pub fn suggest(
    path: &[Dot],
    length_ms: f64,
    reserved: &[Reserved],
    depth: f64,
    mut mint: impl FnMut() -> String,
) -> Suggestions {
    if !length_ms.is_finite() || length_ms <= 0.0 {
        return Suggestions::None(Nothing::NoClip);
    }
    let dots = tidy(path, length_ms);
    if dots.is_empty() {
        return Suggestions::None(Nothing::NoPath);
    }
    // A single sample is a path that never got going - unless that one
    // sample is itself a click, which is a one-frame take of a click.
    if dots.len() == 1 && !dots[0].kind.is_press() {
        return Suggestions::None(Nothing::NoPath);
    }

    let presses = presses(&dots);
    if presses.is_empty() {
        return Suggestions::None(Nothing::NoClicks);
    }

    let mut taken: Vec<Reserved> = reserved.to_vec();
    let mut suggested = Vec::new();
    for cluster in cluster(&presses, MERGE_GAP_MS) {
        let start_ms = (cluster.first_ms - PAD_MS).max(0.0);
        let end_ms = (cluster.last_ms + PAD_MS).min(length_ms);
        if end_ms <= start_ms {
            continue;
        }
        if taken
            .iter()
            .any(|span| end_ms > span.start_ms && start_ms < span.end_ms)
        {
            continue;
        }
        taken.push(Reserved { start_ms, end_ms });
        suggested.push(
            CameraMove {
                mode: CameraMode::Auto,
                ..CameraMove::onto(
                    mint(),
                    start_ms / 1000.0,
                    end_ms / 1000.0,
                    depth,
                    (cluster.x, cluster.y),
                )
            }
            .tidy(),
        );
    }

    if suggested.is_empty() {
        return Suggestions::None(Nothing::NoRoom);
    }
    suggested.sort_by(|left, right| left.start.total_cmp(&right.start));
    Suggestions::Moves(suggested)
}

/// The path, in time order, inside the clip, with the samples that are not
/// numbers dropped.
fn tidy(path: &[Dot], length_ms: f64) -> Vec<Dot> {
    let mut dots: Vec<Dot> = path
        .iter()
        .filter(|dot| dot.at_ms.is_finite() && dot.x.is_finite() && dot.y.is_finite())
        .map(|dot| Dot {
            at_ms: dot.at_ms.clamp(0.0, length_ms),
            x: dot.x.clamp(0.0, 1.0),
            y: dot.y.clamp(0.0, 1.0),
            kind: dot.kind,
        })
        .collect();
    dots.sort_by(|left, right| left.at_ms.total_cmp(&right.at_ms));
    dots
}

/// Every press in the path, each one with how deliberate it looked.
fn presses(dots: &[Dot]) -> Vec<Press> {
    dots.iter()
        .filter(|dot| dot.kind.is_press())
        .map(|dot| Press {
            at_ms: dot.at_ms,
            x: dot.x,
            y: dot.y,
            looked: looked_like(dots, dot),
        })
        .collect()
}

/// How deliberate one press looked, from the pointer's trajectory after it.
///
/// A port of Recordly's `classifyPostClickBehavior`, thresholds and all.
fn looked_like(dots: &[Dot], press: &Dot) -> Deliberate {
    if press.kind == DotKind::PressedTwice {
        return Deliberate::Twice;
    }

    // A release soon after, far enough sideways, is a selection.
    let released = dots.iter().find(|dot| {
        dot.kind == DotKind::Released && dot.at_ms > press.at_ms && dot.at_ms - press.at_ms < 3000.0
    });
    if let Some(up) = released {
        let (across, down) = ((up.x - press.x).abs(), (up.y - press.y).abs());
        if up.at_ms - press.at_ms >= 200.0 && across > 0.03 && across > down * 1.8 {
            return Deliberate::Selection;
        }
    }

    // Otherwise, where the pointer went over the next couple of seconds.
    let after: Vec<&Dot> = dots
        .iter()
        .filter(|dot| {
            dot.at_ms > press.at_ms + 100.0
                && dot.at_ms <= press.at_ms + 2000.0
                && dot.kind == DotKind::Moved
        })
        .collect();
    if after.len() < 3 {
        // Almost nothing after a press: the pointer stayed put, which is
        // what clicking into a text field looks like.
        return Deliberate::TextField;
    }

    let mut furthest = 0.0f64;
    let (mut across, mut down) = (0.0f64, 0.0f64);
    for dot in &after {
        let (dx, dy) = (dot.x - press.x, dot.y - press.y);
        furthest = furthest.max((dx * dx + dy * dy).sqrt());
        across += dx.abs();
        down += dy.abs();
    }
    if furthest < 0.02 {
        return Deliberate::TextField;
    }
    let net_down = after[after.len() - 1].y - press.y;
    if net_down > 0.03 && down > across * 1.5 {
        return Deliberate::Dropdown;
    }
    if across > 0.03 && across > down * 1.8 {
        return Deliberate::Selection;
    }
    Deliberate::Plain
}

/// One moment: the presses that belong together, and where to look.
#[derive(Clone, Copy, PartialEq, Debug)]
struct Cluster {
    first_ms: f64,
    last_ms: f64,
    x: f64,
    y: f64,
}

/// The presses grouped into moments, each no more than `gap_ms` from the
/// last. The focus is the most deliberate-looking press in the group - the
/// earliest of them where several tie, so the frame centres on where the
/// moment began rather than wherever it ended up.
fn cluster(presses: &[Press], gap_ms: f64) -> Vec<Cluster> {
    let mut sorted = presses.to_vec();
    sorted.sort_by(|left, right| left.at_ms.total_cmp(&right.at_ms));

    let mut clusters: Vec<Cluster> = Vec::new();
    let mut group: Vec<Press> = Vec::new();
    for press in sorted {
        let apart = group
            .last()
            .map(|last: &Press| press.at_ms - last.at_ms)
            .unwrap_or(0.0);
        if !group.is_empty() && apart > gap_ms {
            clusters.push(close(&group));
            group.clear();
        }
        group.push(press);
    }
    if !group.is_empty() {
        clusters.push(close(&group));
    }
    clusters
}

/// One group of presses as a moment.
fn close(group: &[Press]) -> Cluster {
    let best = group
        .iter()
        .max_by_key(|press| press.looked)
        .copied()
        .unwrap_or(group[0]);
    Cluster {
        first_ms: group[0].at_ms,
        last_ms: group[group.len() - 1].at_ms,
        x: best.x,
        y: best.y,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Names for suggested moves, so a test can read them.
    fn names() -> impl FnMut() -> String {
        let mut next = 0;
        move || {
            next += 1;
            format!("cam{next}")
        }
    }

    fn moved(at_ms: f64, x: f64, y: f64) -> Dot {
        Dot {
            at_ms,
            x,
            y,
            kind: DotKind::Moved,
        }
    }

    fn pressed(at_ms: f64, x: f64, y: f64) -> Dot {
        Dot {
            at_ms,
            x,
            y,
            kind: DotKind::Pressed,
        }
    }

    /// A still pointer, sampled every 33ms over a span.
    fn still(from_ms: f64, to_ms: f64, x: f64, y: f64) -> Vec<Dot> {
        let mut dots = Vec::new();
        let mut at = from_ms;
        while at <= to_ms {
            dots.push(moved(at, x, y));
            at += 33.0;
        }
        dots
    }

    #[test]
    fn a_clip_with_no_length_suggests_nothing() {
        let path = vec![pressed(100.0, 0.5, 0.5)];
        assert_eq!(
            suggest(&path, 0.0, &[], 2.0, names()),
            Suggestions::None(Nothing::NoClip)
        );
    }

    #[test]
    fn no_path_and_one_lone_sample_are_both_no_path() {
        assert_eq!(
            suggest(&[], 5000.0, &[], 2.0, names()),
            Suggestions::None(Nothing::NoPath)
        );
        assert_eq!(
            suggest(&[moved(10.0, 0.5, 0.5)], 5000.0, &[], 2.0, names()),
            Suggestions::None(Nothing::NoPath)
        );
    }

    #[test]
    fn a_path_with_nothing_clicked_suggests_nothing() {
        let path = still(0.0, 4000.0, 0.5, 0.5);
        assert_eq!(
            suggest(&path, 5000.0, &[], 2.0, names()),
            Suggestions::None(Nothing::NoClicks)
        );
    }

    #[test]
    fn a_dwell_alone_is_not_a_moment() {
        // The pointer parks in one place for two seconds, which is what
        // somebody talking over a recording does. No press, no move.
        let mut path = still(0.0, 1000.0, 0.2, 0.2);
        path.extend(still(1000.0, 3000.0, 0.8, 0.8));
        assert!(matches!(
            suggest(&path, 4000.0, &[], 2.0, names()),
            Suggestions::None(Nothing::NoClicks)
        ));
    }

    #[test]
    fn one_press_becomes_one_move_padded_at_both_ends() {
        let mut path = still(0.0, 5000.0, 0.25, 0.75);
        path.push(pressed(2000.0, 0.25, 0.75));
        let moves = suggest(&path, 6000.0, &[], 2.5, names()).moves();
        assert_eq!(moves.len(), 1);
        assert_eq!(moves[0].start, 1.5);
        assert_eq!(moves[0].end, 2.5);
        assert_eq!(moves[0].depth, 2.5);
        assert_eq!((moves[0].focus_x, moves[0].focus_y), (0.25, 0.75));
        assert_eq!(moves[0].mode, CameraMode::Auto);
        assert_eq!(moves[0].id, "cam1");
    }

    #[test]
    fn the_padding_is_held_inside_the_clip() {
        let path = vec![pressed(100.0, 0.5, 0.5), moved(200.0, 0.5, 0.5)];
        let moves = suggest(&path, 400.0, &[], 2.0, names()).moves();
        assert_eq!(moves.len(), 1);
        assert_eq!(moves[0].start, 0.0);
        assert_eq!(moves[0].end, 0.4);
    }

    #[test]
    fn presses_close_together_are_one_moment() {
        let mut path = still(0.0, 9000.0, 0.5, 0.5);
        for at in [2000.0, 3000.0, 4000.0] {
            path.push(pressed(at, 0.5, 0.5));
        }
        let moves = suggest(&path, 10_000.0, &[], 2.0, names()).moves();
        assert_eq!(moves.len(), 1, "{moves:#?}");
        assert_eq!(moves[0].start, 1.5);
        assert_eq!(moves[0].end, 4.5);
    }

    #[test]
    fn presses_far_apart_are_separate_moments() {
        let mut path = still(0.0, 12_000.0, 0.5, 0.5);
        path.push(pressed(2000.0, 0.2, 0.2));
        path.push(pressed(9000.0, 0.8, 0.8));
        let moves = suggest(&path, 13_000.0, &[], 2.0, names()).moves();
        assert_eq!(moves.len(), 2);
        assert_eq!((moves[0].start, moves[0].end), (1.5, 2.5));
        assert_eq!((moves[1].start, moves[1].end), (8.5, 9.5));
        assert_eq!(moves[0].id, "cam1");
        assert_eq!(moves[1].id, "cam2");
    }

    #[test]
    fn a_gap_of_exactly_the_merge_window_still_joins() {
        let mut path = still(0.0, 9000.0, 0.5, 0.5);
        path.push(pressed(1000.0, 0.5, 0.5));
        path.push(pressed(1000.0 + MERGE_GAP_MS, 0.5, 0.5));
        let moves = suggest(&path, 10_000.0, &[], 2.0, names()).moves();
        assert_eq!(moves.len(), 1);
    }

    #[test]
    fn a_moment_already_covered_by_a_move_is_dropped() {
        let mut path = still(0.0, 12_000.0, 0.5, 0.5);
        path.push(pressed(2000.0, 0.2, 0.2));
        path.push(pressed(9000.0, 0.8, 0.8));
        // The first moment's span is 1.5s..2.5s; a move already there.
        let reserved = [Reserved {
            start_ms: 1000.0,
            end_ms: 3000.0,
        }];
        let moves = suggest(&path, 13_000.0, &reserved, 2.0, names()).moves();
        assert_eq!(moves.len(), 1, "{moves:#?}");
        assert_eq!(moves[0].start, 8.5);
        assert_eq!(moves[0].id, "cam1", "a dropped moment mints no name");
    }

    #[test]
    fn every_moment_covered_is_no_room() {
        let mut path = still(0.0, 5000.0, 0.5, 0.5);
        path.push(pressed(2000.0, 0.5, 0.5));
        let reserved = [Reserved {
            start_ms: 0.0,
            end_ms: 6000.0,
        }];
        assert_eq!(
            suggest(&path, 6000.0, &reserved, 2.0, names()),
            Suggestions::None(Nothing::NoRoom)
        );
    }

    #[test]
    fn two_suggestions_never_overlap_each_other() {
        // Two moments a hair over the merge gap apart, each padded by half
        // a second: the second would reach back into the first.
        let mut path = still(0.0, 9000.0, 0.5, 0.5);
        path.push(pressed(1000.0, 0.5, 0.5));
        path.push(pressed(1000.0 + MERGE_GAP_MS + 1.0, 0.5, 0.5));
        let moves = suggest(&path, 10_000.0, &[], 2.0, names()).moves();
        for pair in moves.windows(2) {
            assert!(
                pair[1].start >= pair[0].end,
                "{:?} overlaps {:?}",
                pair[0],
                pair[1]
            );
        }
    }

    #[test]
    fn the_most_deliberate_press_in_a_cluster_is_what_the_frame_centres_on() {
        // A plain press, then a double press elsewhere in the same moment.
        let mut path = still(0.0, 6000.0, 0.5, 0.5);
        path.push(pressed(2000.0, 0.1, 0.1));
        path.push(Dot {
            at_ms: 2500.0,
            x: 0.9,
            y: 0.9,
            kind: DotKind::PressedTwice,
        });
        let moves = suggest(&path, 7000.0, &[], 2.0, names()).moves();
        assert_eq!(moves.len(), 1);
        assert_eq!((moves[0].focus_x, moves[0].focus_y), (0.9, 0.9));
    }

    #[test]
    fn a_press_followed_by_a_sideways_drag_reads_as_a_selection() {
        let dots = vec![
            pressed(1000.0, 0.3, 0.5),
            Dot {
                at_ms: 1400.0,
                x: 0.5,
                y: 0.505,
                kind: DotKind::Released,
            },
        ];
        assert_eq!(looked_like(&dots, &dots[0]), Deliberate::Selection);
    }

    #[test]
    fn a_press_followed_by_a_slide_downwards_reads_as_a_menu() {
        let mut dots = vec![pressed(1000.0, 0.5, 0.2)];
        for step in 1..=20 {
            dots.push(moved(
                1000.0 + f64::from(step) * 50.0,
                0.5,
                0.2 + f64::from(step) * 0.01,
            ));
        }
        assert_eq!(looked_like(&dots, &dots[0]), Deliberate::Dropdown);
    }

    #[test]
    fn a_press_the_pointer_does_not_move_after_reads_as_a_text_field() {
        let mut dots = vec![pressed(1000.0, 0.5, 0.5)];
        dots.extend(still(1100.0, 2900.0, 0.5, 0.5));
        assert_eq!(looked_like(&dots, &dots[0]), Deliberate::TextField);
    }

    #[test]
    fn a_press_with_nothing_after_it_reads_as_a_text_field() {
        let dots = vec![pressed(1000.0, 0.5, 0.5)];
        assert_eq!(looked_like(&dots, &dots[0]), Deliberate::TextField);
    }

    #[test]
    fn a_press_followed_by_wandering_reads_as_nothing_in_particular() {
        let dots = vec![
            pressed(1000.0, 0.5, 0.5),
            moved(1200.0, 0.55, 0.56),
            moved(1400.0, 0.6, 0.63),
            moved(1600.0, 0.52, 0.60),
            moved(1800.0, 0.58, 0.44),
        ];
        assert_eq!(looked_like(&dots, &dots[0]), Deliberate::Plain);
    }

    #[test]
    fn a_path_out_of_order_or_out_of_frame_is_tidied_first() {
        let path = vec![
            moved(4000.0, 2.0, -1.0),
            pressed(2000.0, 0.5, 0.5),
            moved(f64::NAN, 0.5, 0.5),
            moved(1000.0, 0.5, 0.5),
        ];
        let dots = tidy(&path, 5000.0);
        assert_eq!(dots.len(), 3, "the sample with no time is dropped");
        assert_eq!(dots[0].at_ms, 1000.0);
        assert_eq!((dots[2].x, dots[2].y), (1.0, 0.0));
    }

    #[test]
    fn a_sample_past_the_end_of_the_clip_is_held_at_the_end() {
        let dots = tidy(&[pressed(9000.0, 0.5, 0.5)], 4000.0);
        assert_eq!(dots[0].at_ms, 4000.0);
    }
}
