// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Jareer Abdullah and Concat contributors

//! The screens a take can point at.
//!
//! Enough to fill a picker and to hand a helper a source: each screen's
//! platform identifier, where it sits on the desktop, and which one is the
//! main one. The identifier is the one the capture API wants -
//! `CGDirectDisplayID` on macOS, `HMONITOR` on Windows - so nothing has to
//! be resolved again later.
//!
//! Windows are not listed here: both platforms need a helper for that (the
//! list has to come from the window server, and on macOS reading it needs
//! the same permission capture does), and a picker that cannot list windows
//! can still record a screen.

use crate::source::{Area, Source};

/// One screen.
#[derive(Clone, PartialEq, Debug)]
pub struct Screen {
    /// The platform's identifier for it.
    pub id: u64,
    /// Where it sits on the desktop, in the platform's screen units.
    pub area: Area,
    /// Whether this is the screen the system calls the main one. The
    /// default pick in the UI.
    pub main: bool,
    /// How many pixels to a unit of `area`: 2 on a Retina screen, so a
    /// picker can say what a take will actually be.
    pub density: f64,
}

impl Screen {
    /// The source that records this whole screen.
    pub fn whole(&self) -> Source {
        Source::Screen {
            id: self.id,
            area: self.area,
        }
    }

    /// The take's size in pixels, which is what the editor's frame will be.
    pub fn pixels(&self) -> (u32, u32) {
        (
            (self.area.width * self.density).round().max(0.0) as u32,
            (self.area.height * self.density).round().max(0.0) as u32,
        )
    }
}

/// Every screen attached, main one first. Empty on a platform with no
/// answer, which is a machine that cannot record rather than an error.
pub fn screens() -> Vec<Screen> {
    let mut screens = platform::screens();
    screens.retain(|screen| screen.area.is_usable());
    screens.sort_by_key(|screen| !screen.main);
    screens
}

/// The screen a take defaults to.
pub fn main_screen() -> Option<Screen> {
    screens().into_iter().next()
}

#[cfg(target_vendor = "apple")]
mod platform {
    //! CoreGraphics. `CGGetActiveDisplayList` names the displays that are
    //! drawing, `CGDisplayBounds` places each one in the global space the
    //! pointer is also reported in, and the current mode's pixel width over
    //! those bounds is the density.
    //!
    //! The width has to come from the mode and not from
    //! `CGDisplayPixelsWide`, which reports the mode's *point* width: on a
    //! Retina screen running a scaled resolution those are 1440 points over
    //! a 2880-pixel backing store, and ScreenCaptureKit captures the backing
    //! store. Asking the wrong one makes a picker that promises half the
    //! take it delivers.

    use std::ffi::c_void;

    use super::Screen;
    use crate::source::Area;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CGRect {
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        fn CGGetActiveDisplayList(count: u32, displays: *mut u32, found: *mut u32) -> i32;
        fn CGMainDisplayID() -> u32;
        fn CGDisplayBounds(display: u32) -> CGRect;
        fn CGDisplayCopyDisplayMode(display: u32) -> *const c_void;
        fn CGDisplayModeGetPixelWidth(mode: *const c_void) -> usize;
        fn CGDisplayModeRelease(mode: *const c_void);
    }

    /// The current mode's width in real pixels, or none when the display
    /// will not say.
    fn pixel_width(display: u32) -> Option<usize> {
        // SAFETY: the mode is owned by this call - "Copy" in its name - and
        // released once; it may be null, which is checked before it is read.
        unsafe {
            let mode = CGDisplayCopyDisplayMode(display);
            if mode.is_null() {
                return None;
            }
            let width = CGDisplayModeGetPixelWidth(mode);
            CGDisplayModeRelease(mode);
            (width > 0).then_some(width)
        }
    }

    pub fn screens() -> Vec<Screen> {
        // Sixteen displays is more than any Mac drives; a machine with more
        // records on the first sixteen rather than on none.
        let mut ids = [0u32; 16];
        let mut found = 0u32;
        // SAFETY: the call fills at most `ids.len()` identifiers into the
        // array and writes how many into `found`; a non-zero return means
        // it filled neither.
        let error =
            unsafe { CGGetActiveDisplayList(ids.len() as u32, ids.as_mut_ptr(), &mut found) };
        if error != 0 {
            log::warn!("the display list could not be read ({error})");
            return Vec::new();
        }
        // SAFETY: a call with no arguments and no state.
        let main = unsafe { CGMainDisplayID() };
        ids[..found.min(ids.len() as u32) as usize]
            .iter()
            .map(|&id| {
                // SAFETY: `id` came from the active display list.
                let bounds = unsafe { CGDisplayBounds(id) };
                let density = match pixel_width(id) {
                    Some(wide) if bounds.width > 0.0 => (wide as f64 / bounds.width).max(1.0),
                    _ => 1.0,
                };
                Screen {
                    id: u64::from(id),
                    area: Area {
                        x: bounds.x,
                        y: bounds.y,
                        width: bounds.width,
                        height: bounds.height,
                    },
                    main: id == main,
                    density,
                }
            })
            .collect()
    }
}

#[cfg(windows)]
mod platform {
    //! `EnumDisplayMonitors`, which hands back the `HMONITOR` the Windows
    //! Graphics Capture helper wants and the monitor's rectangle on the
    //! virtual desktop. The rectangle is already in physical pixels, so the
    //! density is one.

    use std::ffi::c_void;

    use windows_sys::Win32::Foundation::{BOOL, LPARAM, RECT, TRUE};
    use windows_sys::Win32::Graphics::Gdi::{
        EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOF_PRIMARY,
    };

    use super::Screen;
    use crate::source::Area;

    pub fn screens() -> Vec<Screen> {
        let mut found: Vec<Screen> = Vec::new();
        // SAFETY: the callback's `LPARAM` is the address of `found`, which
        // outlives the call; the enumeration is synchronous.
        unsafe {
            EnumDisplayMonitors(
                std::ptr::null_mut(),
                std::ptr::null(),
                Some(visit),
                &mut found as *mut Vec<Screen> as LPARAM,
            );
        }
        found
    }

    unsafe extern "system" fn visit(
        monitor: HMONITOR,
        _dc: HDC,
        _rect: *mut RECT,
        state: LPARAM,
    ) -> BOOL {
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..unsafe { std::mem::zeroed() }
        };
        // SAFETY: `monitor` is the handle the enumeration is reporting and
        // `info` has its size filled in, as the call requires.
        if unsafe { GetMonitorInfoW(monitor, &mut info) } != 0 {
            let rect = info.rcMonitor;
            // SAFETY: `state` is the address of the vector `screens` owns.
            let found = unsafe { &mut *(state as *mut Vec<Screen>) };
            found.push(Screen {
                id: monitor as *const c_void as u64,
                area: Area {
                    x: f64::from(rect.left),
                    y: f64::from(rect.top),
                    width: f64::from(rect.right - rect.left),
                    height: f64::from(rect.bottom - rect.top),
                },
                main: info.dwFlags & MONITORINFOF_PRIMARY != 0,
                density: 1.0,
            });
        }
        TRUE
    }
}

#[cfg(not(any(target_vendor = "apple", windows)))]
mod platform {
    //! Nowhere else yet; see [`crate::cursor`]'s own note.

    use super::Screen;

    pub fn screens() -> Vec<Screen> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_screen_listed_can_be_recorded() {
        for screen in screens() {
            assert!(screen.area.is_usable(), "{screen:?}");
            let (width, height) = screen.pixels();
            assert!(width > 0 && height > 0, "{screen:?}");
            assert_eq!(
                screen.whole(),
                Source::Screen {
                    id: screen.id,
                    area: screen.area
                }
            );
        }
    }

    #[test]
    fn the_main_screen_is_the_first_one_listed() {
        let screens = screens();
        if let Some(main) = screens.iter().position(|screen| screen.main) {
            assert_eq!(main, 0, "the main screen is not first: {screens:?}");
        }
        assert_eq!(main_screen().is_some(), !screens.is_empty());
    }
}
