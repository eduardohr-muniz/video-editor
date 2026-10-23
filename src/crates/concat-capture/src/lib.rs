// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Jareer Abdullah and Concat contributors

//! Recording the screen into something the editor can open.
//!
//! A take produces a [`bundle::Bundle`]: one folder holding the picture, the
//! desktop's sound and the microphone as separate files, and the pointer's
//! path as [`cursor::Telemetry`]. Nothing here touches the project document -
//! `concat_project` imports a finished bundle - so a recording can be made,
//! inspected and thrown away without an edit ever happening.
//!
//! The picture is captured by a platform helper, not by this crate: a small
//! binary beside the app that owns one API each - ScreenCaptureKit on macOS,
//! Windows Graphics Capture on Windows. They are spawned, told what to
//! capture as one JSON argument, and driven by three words on their standard
//! input (see [`protocol`]). That boundary is deliberate: screen capture is
//! the part of a recorder that breaks on an OS update, and a helper that
//! crashes loses a take rather than the editor.
//!
//! What this crate owns is everything around the helper: finding it
//! ([`helpers`]), telling it what to point at ([`source`], [`config`]),
//! reading what it says back ([`protocol`]), sampling the pointer while it
//! runs ([`cursor`]), and laying the result out on disk ([`bundle`]).
//! [`session`] is the one type a caller needs.
//!
//! The pointer's path is sampled here rather than in the helper because it
//! is the same work on every platform - ask the system where the pointer is,
//! thirty times a second, in fractions of the captured area - and because
//! the click and the cursor shape arrive from a second helper that only
//! reports changes. A take's telemetry is what makes an automatic zoom
//! possible later: see `concat_project::camera`.

pub mod bundle;
pub mod config;
pub mod cursor;
pub mod error;
pub mod helpers;
pub mod protocol;
pub mod screens;
pub mod session;
pub mod source;

pub use bundle::Bundle;
pub use cursor::{Interaction, Pointer, Sample, Telemetry};
pub use error::Error;
pub use screens::{Screen, main_screen, screens};
pub use session::{Session, Settings, State};
pub use source::{Area, Source};
