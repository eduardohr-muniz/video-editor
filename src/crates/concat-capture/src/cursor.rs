// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Jareer Abdullah and Concat contributors

//! The pointer's path through a take.
//!
//! Where the pointer was, thirty times a second, and what it did: a click,
//! which button, and what shape the system was drawing at the time. Stored
//! as fractions of the captured area, so a take of a window at the far right
//! of a second screen reads the same as a take of the main screen.
//!
//! This is what makes the editor's camera possible. A zoom that finds its
//! own moments has to know when something was clicked, and a zoom that holds
//! the pointer in frame has to know where it went; neither is recoverable
//! from the picture afterwards. Recording it is cheap - two numbers and a
//! tag, thirty times a second - and a take without it is a take that can
//! only ever be zoomed by hand.
//!
//! Three things feed it:
//!
//! - **Position** is sampled in this process, from the platform's own call
//!   for it. It is the same work everywhere and needs no helper.
//! - **Clicks and cursor shapes** arrive as lines from the pointer helper,
//!   which watches the system's event stream. A double click is worked out
//!   here rather than reported: two presses of the same button close enough
//!   in time and in space, which is what the system itself means by it.
//! - **The clock** is the take's, and it stops when the take is paused, so
//!   the path lines up with a picture that has the pause cut out of it.
//!
//! The file is `<video>.cursor.json`, beside the picture, and its shape is
//! the one Recordly writes - the same `version`, the same `samples`, the
//! same spellings - so a take made there opens here with its pointer intact
//! and vice versa. That compatibility is deliberate and worth keeping.

use std::path::{Path, PathBuf};
use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::source::Area;

/// The shape of the file this module writes. Bumped only when a reader that
/// does not know a change would get it wrong.
pub const VERSION: u32 = 2;

/// How often the position is sampled: about thirty times a second, which is
/// finer than any camera move the editor makes from it.
pub const INTERVAL_MS: u64 = 33;

/// As many samples as an hour at thirty a second. A take longer than that
/// keeps its first hour of pointer rather than growing without bound.
pub const MAX_SAMPLES: usize = 60 * 60 * 30;

/// What the pointer did at a sample.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Interaction {
    /// Nothing; the pointer only moved. Written for an ordinary sample.
    Move,
    /// The left button went down.
    Click,
    /// The left button went down twice, close together.
    DoubleClick,
    /// The right button went down.
    RightClick,
    /// The middle button went down.
    MiddleClick,
    /// A button came up.
    #[serde(rename = "mouseup")]
    MouseUp,
}

impl Interaction {
    /// The press of a button, by the number the platforms agree on: 1 left,
    /// 2 right, 3 middle. Anything else is a left click, which is what the
    /// helpers' own normalisation does.
    pub fn of_button(button: u8) -> Interaction {
        match button {
            2 => Interaction::RightClick,
            3 => Interaction::MiddleClick,
            _ => Interaction::Click,
        }
    }

    /// The interaction a file names, or none for a word this build does
    /// not know.
    pub fn named(word: &str) -> Option<Interaction> {
        Some(match word.trim() {
            "move" => Interaction::Move,
            "click" => Interaction::Click,
            "double-click" => Interaction::DoubleClick,
            "right-click" => Interaction::RightClick,
            "middle-click" => Interaction::MiddleClick,
            "mouseup" => Interaction::MouseUp,
            _ => return None,
        })
    }

    /// Whether this is a button going down - the interactions an automatic
    /// zoom looks for.
    pub fn is_press(self) -> bool {
        matches!(
            self,
            Interaction::Click
                | Interaction::DoubleClick
                | Interaction::RightClick
                | Interaction::MiddleClick
        )
    }
}

/// The shape the system was drawing. Drawn again over the zoomed picture,
/// since the captured frames have no pointer in them.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Pointer {
    /// The ordinary arrow.
    Arrow,
    /// The I-beam, over text.
    Text,
    /// The hand, over a link.
    Pointer,
    /// The crosshair.
    Crosshair,
    /// The open hand, over something draggable.
    OpenHand,
    /// The closed hand, dragging.
    ClosedHand,
    /// The horizontal resize arrows.
    ResizeEw,
    /// The vertical resize arrows.
    ResizeNs,
    /// The circle and slash.
    NotAllowed,
}

impl Pointer {
    /// The shape the pointer helper named, or none for a word this build
    /// does not know - a newer helper may report shapes this one cannot
    /// draw, and the arrow is a better answer than a take that fails.
    pub fn named(word: &str) -> Option<Pointer> {
        Some(match word.trim() {
            "arrow" => Pointer::Arrow,
            "text" => Pointer::Text,
            "pointer" => Pointer::Pointer,
            "crosshair" => Pointer::Crosshair,
            "open-hand" => Pointer::OpenHand,
            "closed-hand" => Pointer::ClosedHand,
            "resize-ew" => Pointer::ResizeEw,
            "resize-ns" => Pointer::ResizeNs,
            "not-allowed" => Pointer::NotAllowed,
            _ => return None,
        })
    }
}

/// Where the pointer was at one instant of the take.
#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Sample {
    /// Milliseconds into the take, with any paused time taken out.
    pub time_ms: f64,
    /// Across the captured area, `0..=1`.
    pub cx: f64,
    /// Down the captured area, `0..=1`.
    pub cy: f64,
    /// What happened here, if anything did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interaction_type: Option<Interaction>,
    /// The shape being drawn here, if it was known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor_type: Option<Pointer>,
}

/// A take's whole pointer path.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Telemetry {
    /// The file's shape; see [`VERSION`].
    pub version: u32,
    /// In time order, earliest first.
    pub samples: Vec<Sample>,
}

impl Default for Telemetry {
    fn default() -> Telemetry {
        Telemetry {
            version: VERSION,
            samples: Vec::new(),
        }
    }
}

impl Telemetry {
    /// Where a take's pointer path lives: beside the picture, named after
    /// it. The same path Recordly uses, so either app finds the other's.
    pub fn path_beside(video: &Path) -> PathBuf {
        let mut name = video.as_os_str().to_os_string();
        name.push(".cursor.json");
        PathBuf::from(name)
    }

    /// Reads a path file, tolerating everything a hand-edited or
    /// half-written one can be: a missing file, a truncated one, a sample
    /// whose numbers are nonsense. What survives is in time order and
    /// within the frame. A file that cannot be read at all is an empty
    /// path, never an error: a take whose pointer was lost is still a take.
    pub fn read(path: &Path) -> Telemetry {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Telemetry::default();
        };
        let Ok(parsed) = serde_json::from_str::<Loose>(&text) else {
            log::warn!("pointer path at {} could not be read", path.display());
            return Telemetry::default();
        };
        let mut samples: Vec<Sample> = parsed
            .samples
            .into_iter()
            .take(MAX_SAMPLES)
            .filter_map(|sample| tidy(&sample))
            .collect();
        samples.sort_by(|left, right| left.time_ms.total_cmp(&right.time_ms));
        Telemetry {
            version: parsed.version.unwrap_or(VERSION),
            samples,
        }
    }

    /// Writes the path beside `video`. An empty path writes nothing and
    /// removes any file left by an earlier take of the same name, so a take
    /// never inherits the pointer of the one it replaced.
    pub fn write_beside(&self, video: &Path) -> std::io::Result<Option<PathBuf>> {
        let path = Telemetry::path_beside(video);
        if self.samples.is_empty() {
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            }
            return Ok(None);
        }
        let text = serde_json::to_string(self)
            .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
        std::fs::write(&path, text)?;
        Ok(Some(path))
    }

    /// The samples that fall in `start_ms..end_ms`, with their times moved
    /// so the window begins at zero: what one clip of a longer take has.
    pub fn window(&self, start_ms: f64, end_ms: f64) -> Vec<Sample> {
        self.samples
            .iter()
            .filter(|sample| sample.time_ms >= start_ms && sample.time_ms < end_ms)
            .map(|sample| Sample {
                time_ms: sample.time_ms - start_ms,
                ..*sample
            })
            .collect()
    }
}

/// A path file as it might actually be on disk, before it is tidied.
#[derive(Deserialize)]
struct Loose {
    version: Option<u32>,
    /// Untyped on purpose. A sample is tidied field by field, so one entry
    /// with a string where a number belongs - a hand edit, a writer from
    /// another app - loses that entry and not the file. Typed fields here
    /// would fail the whole parse on the first such sample, which is the
    /// one thing a reader of someone else's file must not do.
    #[serde(default)]
    samples: Vec<serde_json::Value>,
}

/// The sample a value describes, or none when it has no usable time or
/// place. Anything that is not a number where a number belongs is as good
/// as absent.
fn tidy(value: &serde_json::Value) -> Option<Sample> {
    let number = |key: &str| value.get(key)?.as_f64().filter(|value| value.is_finite());
    let word = |key: &str| value.get(key)?.as_str();
    let time_ms = number("timeMs")?.max(0.0);
    let cx = number("cx")?.clamp(0.0, 1.0);
    let cy = number("cy")?.clamp(0.0, 1.0);
    Some(Sample {
        time_ms,
        cx,
        cy,
        interaction_type: word("interactionType").and_then(Interaction::named),
        cursor_type: word("cursorType").and_then(Pointer::named),
    })
}

/// How close in time and in place two presses have to be to read as one
/// double click: what the systems themselves mean by it.
const DOUBLE_CLICK_MS: f64 = 350.0;
/// And how close in place, as a fraction of the captured area's diagonal.
const DOUBLE_CLICK_DISTANCE: f64 = 0.04;

/// Collects a take's pointer path while it runs.
///
/// Driven from outside: the caller samples on a timer and hands over the
/// lines the pointer helper prints. The clock is this type's, and it stops
/// while the take is paused.
pub struct Recorder {
    /// The rectangle positions are measured against.
    area: Area,
    /// When the take began.
    began: Instant,
    /// Milliseconds the take has spent paused, all told.
    paused_ms: f64,
    /// When the current pause began, if it is paused now.
    paused_at: Option<Instant>,
    /// The shape last reported, carried onto each sample.
    shape: Option<Pointer>,
    /// The last left press, for telling a double click from two clicks.
    last_press: Option<(f64, f64, f64)>,
    /// What has been collected.
    samples: Vec<Sample>,
}

impl Recorder {
    /// Starts a path now, measured against `area`.
    pub fn new(area: Area) -> Recorder {
        Recorder {
            area,
            began: Instant::now(),
            paused_ms: 0.0,
            paused_at: None,
            shape: None,
            last_press: None,
            samples: Vec::new(),
        }
    }

    /// Milliseconds into the take, paused time excluded.
    pub fn elapsed_ms(&self) -> f64 {
        let wall = self.began.elapsed().as_secs_f64() * 1000.0;
        let pausing = match self.paused_at {
            Some(since) => since.elapsed().as_secs_f64() * 1000.0,
            None => 0.0,
        };
        (wall - self.paused_ms - pausing).max(0.0)
    }

    /// Whether the clock is stopped.
    pub fn is_paused(&self) -> bool {
        self.paused_at.is_some()
    }

    /// Stops the clock. Samples handed over while paused are dropped, so a
    /// pause leaves no pointer where the picture has no frames.
    pub fn pause(&mut self) {
        if self.paused_at.is_none() {
            self.paused_at = Some(Instant::now());
        }
    }

    /// Starts the clock again.
    pub fn resume(&mut self) {
        if let Some(since) = self.paused_at.take() {
            self.paused_ms += since.elapsed().as_secs_f64() * 1000.0;
        }
    }

    /// Takes one sample of wherever the pointer is now. Call about every
    /// [`INTERVAL_MS`]. Nothing is recorded while paused, once the take has
    /// as many samples as [`MAX_SAMPLES`], or when the platform will not say
    /// where the pointer is.
    pub fn sample(&mut self) {
        let Some((x, y)) = position() else { return };
        let (cx, cy) = self.area.fraction_of(x, y);
        self.push(cx, cy, None);
    }

    /// A line from the pointer helper: `INTERACTION:mousedown[:button]`,
    /// `INTERACTION:mouseup`, or `STATE:<shape>`. Anything else is ignored.
    ///
    /// A press and a release are recorded as their own samples at the
    /// pointer's position at that instant, so a click's moment is exact
    /// rather than rounded to the sampling interval.
    pub fn hear(&mut self, line: &str) {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("STATE:") {
            if let Some(shape) = Pointer::named(rest) {
                self.shape = Some(shape);
            }
            return;
        }
        let Some(rest) = line.strip_prefix("INTERACTION:") else {
            return;
        };
        let mut parts = rest.split(':');
        let what = parts.next().unwrap_or_default();
        let button = parts.next().and_then(|word| word.parse::<u8>().ok());
        match what {
            "mousedown" => self.press(button.unwrap_or(1)),
            "mouseup" => self.release(),
            _ => {}
        }
    }

    /// Records a press of `button` where the pointer is now.
    pub fn press(&mut self, button: u8) {
        let Some((x, y)) = position() else { return };
        let (cx, cy) = self.area.fraction_of(x, y);
        let at = self.elapsed_ms();
        let mut interaction = Interaction::of_button(button);
        if interaction == Interaction::Click {
            if let Some((was, was_cx, was_cy)) = self.last_press {
                let apart = ((cx - was_cx).powi(2) + (cy - was_cy).powi(2)).sqrt();
                if at - was <= DOUBLE_CLICK_MS && apart <= DOUBLE_CLICK_DISTANCE {
                    interaction = Interaction::DoubleClick;
                }
            }
            self.last_press = Some((at, cx, cy));
        }
        self.push(cx, cy, Some(interaction));
    }

    /// Records a button coming up where the pointer is now.
    pub fn release(&mut self) {
        let Some((x, y)) = position() else { return };
        let (cx, cy) = self.area.fraction_of(x, y);
        self.push(cx, cy, Some(Interaction::MouseUp));
    }

    /// A copy of what has been collected, for a take that is being
    /// finished while the reader threads are still running.
    pub fn samples_so_far(&self) -> Vec<Sample> {
        self.samples.clone()
    }

    /// The path collected so far, in time order.
    pub fn finish(self) -> Telemetry {
        Telemetry {
            version: VERSION,
            samples: self.samples,
        }
    }

    /// How many samples have been collected. For the diagnostics.
    pub fn len(&self) -> usize {
        self.samples.len()
    }

    /// Whether nothing has been collected.
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// Adds a sample at the take's current instant, unless the take is
    /// paused or already full.
    fn push(&mut self, cx: f64, cy: f64, interaction: Option<Interaction>) {
        if self.is_paused() || self.samples.len() >= MAX_SAMPLES {
            return;
        }
        self.samples.push(Sample {
            time_ms: self.elapsed_ms(),
            cx,
            cy,
            interaction_type: interaction,
            cursor_type: self.shape,
        });
    }
}

/// Where the pointer is on the desktop, in the platform's screen units with
/// a top-left origin - the same units an [`Area`] is in. None on a platform
/// with no answer, which makes a take without a pointer path rather than a
/// take that fails.
pub fn position() -> Option<(f64, f64)> {
    platform::position()
}

#[cfg(target_vendor = "apple")]
mod platform {
    //! CoreGraphics. `CGEventCreate(NULL)` makes an event that carries the
    //! current pointer location, which is the documented way to ask for it
    //! outside an event handler; its location is in global display
    //! coordinates with a top-left origin, unlike AppKit's.

    use std::ffi::c_void;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CGPoint {
        x: f64,
        y: f64,
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGEventCreate(source: *const c_void) -> *const c_void;
        fn CGEventGetLocation(event: *const c_void) -> CGPoint;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFRelease(value: *const c_void);
    }

    pub fn position() -> Option<(f64, f64)> {
        // SAFETY: CGEventCreate with a null source is the documented way to
        // make an event from the current state; it returns null on failure,
        // which is checked before the event is read. The event is owned here
        // and released once.
        unsafe {
            let event = CGEventCreate(std::ptr::null());
            if event.is_null() {
                return None;
            }
            let point = CGEventGetLocation(event);
            CFRelease(event);
            (point.x.is_finite() && point.y.is_finite()).then_some((point.x, point.y))
        }
    }
}

#[cfg(windows)]
mod platform {
    //! `GetCursorPos`, in physical pixels on the virtual desktop.

    use windows_sys::Win32::Foundation::POINT;
    use windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos;

    pub fn position() -> Option<(f64, f64)> {
        let mut point = POINT { x: 0, y: 0 };
        // SAFETY: the call fills a POINT this stack frame owns, and its
        // return says whether it did.
        let ok = unsafe { GetCursorPos(&mut point) };
        (ok != 0).then(|| (f64::from(point.x), f64::from(point.y)))
    }
}

#[cfg(not(any(target_vendor = "apple", windows)))]
mod platform {
    //! Nowhere else yet. X11 and the Wayland portals are both a protocol
    //! conversation rather than a call, and belong with a Linux capture
    //! helper rather than ahead of one.

    pub fn position() -> Option<(f64, f64)> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn area() -> Area {
        Area::sized(1000.0, 500.0)
    }

    #[test]
    fn a_button_number_is_an_interaction() {
        assert_eq!(Interaction::of_button(1), Interaction::Click);
        assert_eq!(Interaction::of_button(2), Interaction::RightClick);
        assert_eq!(Interaction::of_button(3), Interaction::MiddleClick);
        assert_eq!(Interaction::of_button(9), Interaction::Click);
    }

    #[test]
    fn only_a_button_going_down_is_a_press() {
        assert!(Interaction::Click.is_press());
        assert!(Interaction::DoubleClick.is_press());
        assert!(Interaction::RightClick.is_press());
        assert!(!Interaction::MouseUp.is_press());
        assert!(!Interaction::Move.is_press());
    }

    #[test]
    fn the_interaction_names_are_the_ones_on_disk() {
        let names = [
            (Interaction::Move, "\"move\""),
            (Interaction::Click, "\"click\""),
            (Interaction::DoubleClick, "\"double-click\""),
            (Interaction::RightClick, "\"right-click\""),
            (Interaction::MiddleClick, "\"middle-click\""),
            (Interaction::MouseUp, "\"mouseup\""),
        ];
        for (interaction, text) in names {
            assert_eq!(serde_json::to_string(&interaction).unwrap(), text);
        }
    }

    #[test]
    fn the_shape_names_are_the_ones_on_disk() {
        for (shape, text) in [
            (Pointer::Arrow, "\"arrow\""),
            (Pointer::OpenHand, "\"open-hand\""),
            (Pointer::ResizeEw, "\"resize-ew\""),
            (Pointer::NotAllowed, "\"not-allowed\""),
        ] {
            assert_eq!(serde_json::to_string(&shape).unwrap(), text);
            assert_eq!(Pointer::named(text.trim_matches('"')), Some(shape));
        }
        assert_eq!(Pointer::named("hourglass"), None);
    }

    #[test]
    fn the_path_file_sits_beside_the_picture() {
        assert_eq!(
            Telemetry::path_beside(Path::new("/takes/one.mp4")),
            PathBuf::from("/takes/one.mp4.cursor.json")
        );
    }

    #[test]
    fn a_file_that_is_not_there_is_an_empty_path() {
        let telemetry = Telemetry::read(Path::new("/concat-capture-nowhere/one.mp4.cursor.json"));
        assert!(telemetry.samples.is_empty());
        assert_eq!(telemetry.version, VERSION);
    }

    #[test]
    fn a_sample_with_nonsense_in_it_is_dropped_and_the_rest_kept() {
        let text = r#"{"version":2,"samples":[
            {"timeMs":100,"cx":0.5,"cy":0.5,"interactionType":"click"},
            {"timeMs":null,"cx":0.1,"cy":0.1},
            {"cx":0.2,"cy":0.2},
            {"timeMs":50,"cx":4,"cy":-2,"cursorType":"text"},
            {"timeMs":"soon","cx":0.3,"cy":0.3}
        ]}"#;
        let parsed: Loose = serde_json::from_str(text).expect("the file reads");
        let kept: Vec<Sample> = parsed.samples.iter().filter_map(tidy).collect();
        assert_eq!(kept.len(), 2);
        assert_eq!(kept[0].interaction_type, Some(Interaction::Click));
        // Held into the frame, not dropped.
        assert_eq!((kept[1].cx, kept[1].cy), (1.0, 0.0));
        assert_eq!(kept[1].cursor_type, Some(Pointer::Text));
    }

    #[test]
    fn an_unknown_tag_leaves_the_sample_standing() {
        let text = r#"{"samples":[{"timeMs":1,"cx":0.5,"cy":0.5,"interactionType":"triple-click","cursorType":"zoom-in"}]}"#;
        let parsed: Loose = serde_json::from_str(text).expect("the file reads");
        let sample = tidy(&parsed.samples[0]).expect("a sample");
        assert_eq!(sample.interaction_type, None);
        assert_eq!(sample.cursor_type, None);
    }

    #[test]
    fn a_path_is_written_and_read_back_whole() {
        let dir = std::env::temp_dir().join(format!("concat-capture-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a temporary folder");
        let video = dir.join("one.mp4");
        let telemetry = Telemetry {
            version: VERSION,
            samples: vec![
                Sample {
                    time_ms: 0.0,
                    cx: 0.25,
                    cy: 0.75,
                    interaction_type: None,
                    cursor_type: Some(Pointer::Arrow),
                },
                Sample {
                    time_ms: 120.0,
                    cx: 0.5,
                    cy: 0.5,
                    interaction_type: Some(Interaction::DoubleClick),
                    cursor_type: None,
                },
            ],
        };
        let written = telemetry
            .write_beside(&video)
            .expect("written")
            .expect("a path");
        assert_eq!(Telemetry::read(&written), telemetry);

        // An empty path takes the old file with it.
        Telemetry::default().write_beside(&video).expect("removed");
        assert!(!written.exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_window_of_a_path_starts_at_zero() {
        let telemetry = Telemetry {
            version: VERSION,
            samples: (0..10)
                .map(|step| Sample {
                    time_ms: f64::from(step) * 100.0,
                    cx: 0.5,
                    cy: 0.5,
                    interaction_type: None,
                    cursor_type: None,
                })
                .collect(),
        };
        let window = telemetry.window(300.0, 600.0);
        assert_eq!(window.len(), 3);
        assert_eq!(window[0].time_ms, 0.0);
        assert_eq!(window[2].time_ms, 200.0);
    }

    #[test]
    fn the_helper_s_lines_are_heard() {
        let mut recorder = Recorder::new(area());
        recorder.hear("STATE:text");
        assert_eq!(recorder.shape, Some(Pointer::Text));
        recorder.hear("STATE:nonsense");
        assert_eq!(recorder.shape, Some(Pointer::Text));
        recorder.hear("INTERACTION:scrolled");
        recorder.hear("not ours at all");
        assert!(recorder.is_empty());
    }

    #[test]
    fn a_paused_take_collects_nothing_and_its_clock_stops() {
        let mut recorder = Recorder::new(area());
        recorder.pause();
        assert!(recorder.is_paused());
        recorder.push(0.5, 0.5, None);
        assert!(recorder.is_empty());
        let stopped = recorder.elapsed_ms();
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert!(
            (recorder.elapsed_ms() - stopped).abs() < 1.0,
            "the clock moved while paused"
        );
        recorder.resume();
        assert!(!recorder.is_paused());
        recorder.push(0.5, 0.5, None);
        assert_eq!(recorder.len(), 1);
    }

    #[test]
    fn a_full_take_stops_collecting() {
        let mut recorder = Recorder::new(area());
        recorder.samples = vec![
            Sample {
                time_ms: 0.0,
                cx: 0.0,
                cy: 0.0,
                interaction_type: None,
                cursor_type: None,
            };
            MAX_SAMPLES
        ];
        recorder.push(0.5, 0.5, None);
        assert_eq!(recorder.len(), MAX_SAMPLES);
    }

    #[test]
    fn two_presses_close_together_are_one_double_click() {
        let mut recorder = Recorder::new(area());
        // The press path needs the platform's pointer, which a test machine
        // may not have (a headless CI runner). The rule itself is what is
        // under test, so it is driven through push with the same arithmetic.
        let double = |at: f64, was: Option<(f64, f64, f64)>, cx: f64, cy: f64| {
            was.is_some_and(|(was_at, was_cx, was_cy)| {
                let apart = ((cx - was_cx).powi(2) + (cy - was_cy).powi(2)).sqrt();
                at - was_at <= DOUBLE_CLICK_MS && apart <= DOUBLE_CLICK_DISTANCE
            })
        };
        recorder.last_press = Some((1000.0, 0.5, 0.5));
        assert!(
            double(1200.0, recorder.last_press, 0.51, 0.5),
            "same place, soon after"
        );
        assert!(!double(1500.0, recorder.last_press, 0.5, 0.5), "too late");
        assert!(!double(1100.0, recorder.last_press, 0.7, 0.5), "too far");
    }
}
