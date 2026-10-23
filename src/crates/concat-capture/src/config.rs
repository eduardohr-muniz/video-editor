// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Jareer Abdullah and Concat contributors

//! The one argument a capture helper is started with.
//!
//! Both helpers take their whole configuration as a single JSON string in
//! `argv[1]` and nothing else - no flags, no environment, no file. They do
//! not agree on the spelling: ScreenCaptureKit's helper wants
//! `capturesSystemAudio` and `systemAudioOutputPath`, the Windows one wants
//! `captureSystemAudio` and `audioOutputPath`, and each names a screen in
//! its own platform's identifier. So the two shapes are built separately
//! here, from one [`Request`], and the difference stops at this file.
//!
//! Anything the request leaves out is left out of the JSON rather than sent
//! as null: both helpers read their fields as optional and a missing key is
//! the documented way to say "do not".

use std::path::{Path, PathBuf};

use serde_json::{Map, Value, json};

use crate::source::Source;

/// What a take asks of a helper.
#[derive(Clone, PartialEq, Debug)]
pub struct Request {
    /// What to point at.
    pub source: Source,
    /// Frames a second asked of the capture API. Both helpers treat this as
    /// a ceiling: a still screen produces fewer.
    pub fps: u32,
    /// Where the picture goes.
    pub video: PathBuf,
    /// Where the desktop's sound goes, when it is wanted.
    pub system_audio: Option<PathBuf>,
    /// Where the microphone goes, when it is wanted.
    pub microphone: Option<PathBuf>,
    /// Which microphone, by the platform's identifier for it. The default
    /// input when absent.
    pub microphone_device: Option<String>,
    /// Processes whose windows are left out of the picture - the app's own,
    /// so the recording panel does not appear in the recording. macOS only;
    /// Windows Graphics Capture has no equivalent.
    pub excluded_processes: Vec<i32>,
}

impl Request {
    /// The JSON for the platform this build runs on.
    pub fn to_json(&self) -> String {
        let value = if cfg!(windows) {
            self.windows()
        } else {
            self.apple()
        };
        // An unserialisable value is not reachable: every leaf is a number,
        // a string or a bool.
        serde_json::to_string(&value).unwrap_or_else(|_| "{}".to_owned())
    }

    /// ScreenCaptureKit's spelling.
    fn apple(&self) -> Value {
        let mut config = Map::new();
        config.insert("fps".into(), json!(self.fps));
        config.insert("outputPath".into(), path(&self.video));
        match &self.source {
            Source::Screen { id, .. } => {
                config.insert("displayId".into(), json!(id));
            }
            Source::Window { id, area } => {
                config.insert("windowId".into(), json!(id));
                config.insert("windowX".into(), json!(area.x));
                config.insert("windowY".into(), json!(area.y));
                config.insert("windowWidth".into(), json!(area.width));
                config.insert("windowHeight".into(), json!(area.height));
            }
            Source::Region { screen, area } => {
                config.insert("displayId".into(), json!(screen));
                config.insert("regionX".into(), json!(area.x));
                config.insert("regionY".into(), json!(area.y));
                config.insert("regionWidth".into(), json!(area.width));
                config.insert("regionHeight".into(), json!(area.height));
            }
        }
        if let Some(audio) = &self.system_audio {
            config.insert("capturesSystemAudio".into(), json!(true));
            config.insert("systemAudioOutputPath".into(), path(audio));
        }
        if let Some(microphone) = &self.microphone {
            config.insert("capturesMicrophone".into(), json!(true));
            config.insert("microphoneOutputPath".into(), path(microphone));
            if let Some(device) = &self.microphone_device {
                config.insert("microphoneDeviceId".into(), json!(device));
            }
        }
        if !self.excluded_processes.is_empty() {
            config.insert("excludedProcessIds".into(), json!(self.excluded_processes));
        }
        Value::Object(config)
    }

    /// Windows Graphics Capture's spelling.
    fn windows(&self) -> Value {
        let mut config = Map::new();
        config.insert("fps".into(), json!(self.fps));
        config.insert("outputPath".into(), path(&self.video));
        match &self.source {
            // A region is captured as its whole screen and cropped by the
            // editor: the WGC helper takes no sub-rectangle, and a take that
            // keeps the whole screen can always be cropped later, where a
            // take that lost it cannot be widened.
            Source::Screen { id, area } | Source::Region { screen: id, area } => {
                config.insert("displayId".into(), json!(id));
                config.insert("displayX".into(), json!(area.x.round() as i64));
                config.insert("displayY".into(), json!(area.y.round() as i64));
                config.insert("displayW".into(), json!(area.width.round() as i64));
                config.insert("displayH".into(), json!(area.height.round() as i64));
            }
            Source::Window { id, .. } => {
                config.insert("windowHandle".into(), json!(id));
            }
        }
        if let Some(audio) = &self.system_audio {
            config.insert("captureSystemAudio".into(), json!(true));
            config.insert("audioOutputPath".into(), path(audio));
        }
        if let Some(microphone) = &self.microphone {
            config.insert("captureMic".into(), json!(true));
            config.insert("micOutputPath".into(), path(microphone));
            if let Some(device) = &self.microphone_device {
                config.insert("micDeviceId".into(), json!(device));
            }
        }
        Value::Object(config)
    }
}

/// A path as the helpers take it: a string, in the platform's own spelling.
/// A path that is not valid Unicode is lost here rather than at the helper,
/// which would read it as an empty string and write to the working folder.
fn path(path: &Path) -> Value {
    json!(path.to_string_lossy())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::source::Area;

    fn request(source: Source) -> Request {
        Request {
            source,
            fps: 60,
            video: PathBuf::from("/takes/one.mp4"),
            system_audio: None,
            microphone: None,
            microphone_device: None,
            excluded_processes: Vec::new(),
        }
    }

    fn area() -> Area {
        Area {
            x: 10.0,
            y: 20.0,
            width: 1920.0,
            height: 1080.0,
        }
    }

    #[test]
    fn a_screen_is_named_by_its_identifier() {
        let config = request(Source::Screen {
            id: 7,
            area: area(),
        })
        .apple();
        assert_eq!(config["displayId"], json!(7));
        assert_eq!(config["outputPath"], json!("/takes/one.mp4"));
        assert_eq!(config["fps"], json!(60));
        assert!(config.get("regionX").is_none());
    }

    #[test]
    fn a_region_is_a_screen_and_a_rectangle() {
        let config = request(Source::Region {
            screen: 3,
            area: area(),
        })
        .apple();
        assert_eq!(config["displayId"], json!(3));
        assert_eq!(config["regionX"], json!(10.0));
        assert_eq!(config["regionWidth"], json!(1920.0));
    }

    #[test]
    fn a_window_carries_the_bounds_it_had_at_the_start() {
        let config = request(Source::Window {
            id: 42,
            area: area(),
        })
        .apple();
        assert_eq!(config["windowId"], json!(42));
        assert_eq!(config["windowHeight"], json!(1080.0));
        assert!(config.get("displayId").is_none());
    }

    #[test]
    fn sound_is_left_out_rather_than_switched_off() {
        let config = request(Source::Screen {
            id: 1,
            area: area(),
        })
        .apple();
        assert!(config.get("capturesSystemAudio").is_none());
        assert!(config.get("capturesMicrophone").is_none());
    }

    #[test]
    fn sound_names_its_own_file_on_each_platform() {
        let mut asked = request(Source::Screen {
            id: 1,
            area: area(),
        });
        asked.system_audio = Some(PathBuf::from("/takes/one.system.wav"));
        asked.microphone = Some(PathBuf::from("/takes/one.mic.wav"));
        asked.microphone_device = Some("mic-1".to_owned());

        let apple = asked.apple();
        assert_eq!(apple["capturesSystemAudio"], json!(true));
        assert_eq!(
            apple["systemAudioOutputPath"],
            json!("/takes/one.system.wav")
        );
        assert_eq!(apple["microphoneOutputPath"], json!("/takes/one.mic.wav"));
        assert_eq!(apple["microphoneDeviceId"], json!("mic-1"));

        let windows = asked.windows();
        assert_eq!(windows["captureSystemAudio"], json!(true));
        assert_eq!(windows["audioOutputPath"], json!("/takes/one.system.wav"));
        assert_eq!(windows["micOutputPath"], json!("/takes/one.mic.wav"));
        assert_eq!(windows["micDeviceId"], json!("mic-1"));
    }

    #[test]
    fn windows_captures_a_region_as_its_whole_screen() {
        let config = request(Source::Region {
            screen: 5,
            area: area(),
        })
        .windows();
        assert_eq!(config["displayId"], json!(5));
        assert_eq!(config["displayW"], json!(1920));
        assert!(config.get("regionX").is_none());
    }

    #[test]
    fn windows_names_a_window_by_its_handle_alone() {
        let config = request(Source::Window {
            id: 99,
            area: area(),
        })
        .windows();
        assert_eq!(config["windowHandle"], json!(99));
        assert!(config.get("displayId").is_none());
    }

    #[test]
    fn the_app_s_own_windows_can_be_kept_out_of_the_picture() {
        let mut asked = request(Source::Screen {
            id: 1,
            area: area(),
        });
        asked.excluded_processes = vec![501, 502];
        assert_eq!(asked.apple()["excludedProcessIds"], json!([501, 502]));
    }

    #[test]
    fn the_json_is_one_object() {
        let text = request(Source::Screen {
            id: 1,
            area: area(),
        })
        .to_json();
        assert!(text.starts_with('{') && text.ends_with('}'), "{text}");
        let parsed: Value = serde_json::from_str(&text).expect("valid JSON");
        assert_eq!(parsed["fps"], json!(60));
    }
}
