// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Jareer Abdullah and Concat contributors

//! What a take points at: a screen, a window, or a rectangle of a screen.
//!
//! The identifiers are the platform's own and are only ever passed back to
//! it: a `CGDirectDisplayID` and a window number on macOS, an `HMONITOR` and
//! an `HWND` on Windows. Nothing here interprets them, so a source picked in
//! the UI survives the trip to the helper unchanged.
//!
//! An [`Area`] is in the platform's own screen units - points on macOS,
//! physical pixels on Windows - because that is what both capture APIs take.
//! The pointer's path is normalised against the same rectangle while the
//! take runs (see [`crate::cursor`]), which is what lets a zoom follow the
//! pointer in a frame of a window that is not at the screen's origin.

use serde::{Deserialize, Serialize};

/// A rectangle of a screen, in the platform's screen units, top-left origin.
#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
pub struct Area {
    /// Distance from the left of the screen, or of the desktop on Windows.
    pub x: f64,
    /// Distance from the top.
    pub y: f64,
    /// Width. Zero or less is an area nothing can be captured from.
    pub width: f64,
    /// Height.
    pub height: f64,
}

impl Area {
    /// The rectangle at the origin of the given size.
    pub fn sized(width: f64, height: f64) -> Area {
        Area {
            x: 0.0,
            y: 0.0,
            width,
            height,
        }
    }

    /// Whether a frame can come from this: a positive, finite rectangle.
    pub fn is_usable(&self) -> bool {
        self.x.is_finite()
            && self.y.is_finite()
            && self.width.is_finite()
            && self.height.is_finite()
            && self.width > 0.0
            && self.height > 0.0
    }

    /// Where a point of the screen falls inside this rectangle, as fractions
    /// of its width and height, held to `0..=1`. The pointer's path is
    /// stored in these, so it means the same thing whatever was captured.
    ///
    /// A rectangle nothing can be captured from puts everything at its
    /// centre, which is the one answer that cannot send a zoom off-frame.
    pub fn fraction_of(&self, x: f64, y: f64) -> (f64, f64) {
        if !self.is_usable() || !x.is_finite() || !y.is_finite() {
            return (0.5, 0.5);
        }
        let cx = (x - self.x) / self.width;
        let cy = (y - self.y) / self.height;
        (cx.clamp(0.0, 1.0), cy.clamp(0.0, 1.0))
    }
}

/// What to capture.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Source {
    /// A whole screen, by the platform's identifier for it. `area` is where
    /// that screen sits on the desktop, which the pointer's path is
    /// normalised against.
    Screen {
        /// `CGDirectDisplayID` on macOS, `HMONITOR` on Windows.
        id: u64,
        /// The screen's rectangle on the desktop.
        area: Area,
    },
    /// One window, wherever it moves to. `area` is where it was when the
    /// take began - the best guess available for normalising the pointer,
    /// since a window that is dragged mid-take takes its own frame with it.
    Window {
        /// The window number on macOS, the `HWND` on Windows.
        id: u64,
        /// The window's rectangle on the desktop at the start of the take.
        area: Area,
    },
    /// A fixed rectangle of one screen.
    Region {
        /// The screen the rectangle is cut from.
        screen: u64,
        /// The rectangle, on the desktop.
        area: Area,
    },
}

impl Source {
    /// The rectangle the pointer's path is measured against.
    pub fn area(&self) -> Area {
        match *self {
            Source::Screen { area, .. }
            | Source::Window { area, .. }
            | Source::Region { area, .. } => area,
        }
    }

    /// A name for logs and for the bundle's manifest.
    pub fn describe(&self) -> String {
        match self {
            Source::Screen { id, .. } => format!("screen {id}"),
            Source::Window { id, .. } => format!("window {id}"),
            Source::Region { screen, area } => {
                format!("{}x{} of screen {screen}", area.width, area.height)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_point_is_a_fraction_of_the_captured_area() {
        let area = Area {
            x: 100.0,
            y: 50.0,
            width: 400.0,
            height: 200.0,
        };
        assert_eq!(area.fraction_of(300.0, 150.0), (0.5, 0.5));
        assert_eq!(area.fraction_of(100.0, 50.0), (0.0, 0.0));
        assert_eq!(area.fraction_of(500.0, 250.0), (1.0, 1.0));
    }

    #[test]
    fn a_point_outside_the_area_is_held_to_its_edge() {
        let area = Area::sized(100.0, 100.0);
        assert_eq!(area.fraction_of(-40.0, 400.0), (0.0, 1.0));
    }

    #[test]
    fn an_unusable_area_puts_everything_at_the_centre() {
        let area = Area::sized(0.0, 0.0);
        assert_eq!(area.fraction_of(10.0, 10.0), (0.5, 0.5));
        assert!(!area.is_usable());
    }

    #[test]
    fn a_point_that_is_not_a_number_is_the_centre() {
        let area = Area::sized(100.0, 100.0);
        assert_eq!(area.fraction_of(f64::NAN, 10.0), (0.5, 0.5));
    }
}
