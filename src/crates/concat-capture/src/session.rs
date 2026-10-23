// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Jareer Abdullah and Concat contributors

//! One take, from Record to a finished bundle.
//!
//! A [`Session`] owns the two helper processes - the screen recorder and the
//! pointer monitor - and the threads that read them. Three threads, each
//! with one job and none of them touching the UI:
//!
//! - one reads the screen helper's output and turns each line into a
//!   [`Report`] on a channel, so a caller never blocks on a pipe;
//! - one reads the pointer helper's output into the pointer path;
//! - one samples the pointer's position on a timer.
//!
//! The caller drives it from its own loop: [`Session::poll`] drains what has
//! been said, [`Session::pause`] and [`Session::resume`] pass a word to the
//! helper, and [`Session::finish`] asks it to stop, waits for the file to be
//! finalised, and writes the pointer path and the manifest.
//!
//! The take's clock starts when the helper says `Recording started`, not
//! when it was spawned: ScreenCaptureKit takes a moment to wake a display up
//! and the first frames are not written before it does. Starting the
//! pointer path at the same word is what keeps a click at 4.2 seconds in the
//! path and a click at 4.2 seconds in the picture the same click.
//!
//! Nothing here is asynchronous and nothing here owns a runtime. A take is
//! two processes and a timer, and the editor's own event loop is the only
//! loop it needs.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use crate::bundle::{self, Bundle};
use crate::config::Request;
use crate::cursor::{self, Recorder, Telemetry};
use crate::error::{Error, Result};
use crate::helpers::Helper;
use crate::protocol::{Command as Word, Report};
use crate::source::Source;

/// What a take is asked for beyond its source.
#[derive(Clone, PartialEq, Debug)]
pub struct Settings {
    /// Frames a second asked of the capture API.
    pub fps: u32,
    /// Keep the desktop's sound.
    pub system_audio: bool,
    /// Keep the microphone.
    pub microphone: bool,
    /// Which microphone, by the platform's identifier; the default input
    /// when absent.
    pub microphone_device: Option<String>,
    /// Follow the pointer: sample where it goes and what it clicks, so the
    /// editor can zoom by itself afterwards. Off makes a plain screen
    /// recording.
    pub follow_pointer: bool,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            fps: 60,
            system_audio: true,
            microphone: false,
            microphone_device: None,
            follow_pointer: true,
        }
    }
}

/// Where a take is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    /// Spawned; the capture API has not said it is running yet.
    Starting,
    /// Frames are being written.
    Running,
    /// Frames are not being written, and the clock has stopped.
    Paused,
    /// The helper has been asked to stop and has not finished yet.
    Stopping,
    /// The file is finalised.
    Stopped,
    /// The take ended badly. Whatever was written is still on disk.
    Failed,
}

/// How long to wait for the capture API to wake up before giving up on a
/// take. ScreenCaptureKit's own first-frame latency is a few hundred
/// milliseconds; twelve seconds is the point at which something is wrong
/// rather than slow.
const START_TIMEOUT: Duration = Duration::from_secs(12);

/// How long to wait for a stopped helper to finalise its file. An hour of
/// 4K is a long write, but the helper writes as it goes, so this is the
/// trailer and not the file.
const STOP_TIMEOUT: Duration = Duration::from_secs(30);

/// A take in progress.
pub struct Session {
    /// Where it is being written.
    bundle: Bundle,
    /// The screen helper.
    screen: Child,
    /// The pointer helper, when the take follows the pointer.
    pointer: Option<Child>,
    /// What the screen helper has said.
    reports: mpsc::Receiver<Report>,
    /// The pointer's path so far. Shared with the reader and the sampler.
    path: Arc<Mutex<Recorder>>,
    /// Cleared to stop the sampler thread.
    sampling: Arc<AtomicBool>,
    /// Where the take is.
    state: State,
    /// What ended it, when it ended badly.
    failure: Option<String>,
}

impl Session {
    /// Starts a take. Returns once both helpers are spawned, which is before
    /// the first frame: wait for [`Session::wait_until_running`], or watch
    /// for [`Report::Started`] from [`Session::poll`], before telling anyone
    /// the recording has begun.
    ///
    /// A take that cannot start leaves no folder behind.
    pub fn start(recordings: &Path, source: Source, settings: &Settings) -> Result<Session> {
        let screen_helper = Helper::Screen.locate()?;
        let bundle = Bundle::create(recordings, &source, settings.fps)?;

        let request = Request {
            source: source.clone(),
            fps: settings.fps.clamp(1, 240),
            video: bundle.video(),
            system_audio: settings
                .system_audio
                .then(|| bundle.folder.join(bundle::SYSTEM_AUDIO)),
            microphone: settings
                .microphone
                .then(|| bundle.folder.join(bundle::MICROPHONE)),
            microphone_device: settings.microphone_device.clone(),
            // The app's own windows: the recording panel must not appear in
            // the recording. macOS only, and the helper ignores it elsewhere.
            excluded_processes: vec![std::process::id() as i32],
        };

        let started = Session::spawn(&screen_helper, &request.to_json(), &bundle.folder);
        let screen = match started {
            Ok(child) => child,
            Err(error) => {
                // A take that never began should not leave a folder in the
                // library for someone to wonder about.
                bundle.discard().ok();
                return Err(error);
            }
        };

        let (sender, reports) = mpsc::channel();
        let path = Arc::new(Mutex::new(Recorder::new(source.area())));
        let sampling = Arc::new(AtomicBool::new(false));

        let mut session = Session {
            bundle,
            screen,
            pointer: None,
            reports,
            path,
            sampling,
            state: State::Starting,
            failure: None,
        };
        session.read_helper(sender);
        if settings.follow_pointer {
            session.start_following();
        }
        Ok(session)
    }

    /// Where the take is being written. Useful before it has finished - the
    /// folder is shown in the UI while recording.
    pub fn folder(&self) -> &Path {
        &self.bundle.folder
    }

    /// Where the take is.
    pub fn state(&self) -> State {
        self.state
    }

    /// Milliseconds of take so far, pauses excluded. Zero until the capture
    /// API says it is running.
    pub fn elapsed_ms(&self) -> f64 {
        match self.state {
            State::Starting => 0.0,
            _ => self
                .path
                .lock()
                .map(|path| path.elapsed_ms())
                .unwrap_or(0.0),
        }
    }

    /// How many pointer samples have been collected.
    pub fn pointer_samples(&self) -> usize {
        self.path.lock().map(|path| path.len()).unwrap_or(0)
    }

    /// Everything the helper has said since the last call, with the take's
    /// state brought up to date. Call it from the UI's own tick; it never
    /// blocks.
    pub fn poll(&mut self) -> Vec<Report> {
        let mut reports = Vec::new();
        while let Ok(report) = self.reports.try_recv() {
            self.absorb(&report);
            reports.push(report);
        }
        reports
    }

    /// Blocks until the capture API is running, or until it is clear it will
    /// not be. The reports read along the way are absorbed, so a caller that
    /// uses this still sees the right state afterwards.
    pub fn wait_until_running(&mut self) -> Result<()> {
        let deadline = Instant::now() + START_TIMEOUT;
        loop {
            match self
                .reports
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            {
                Ok(report) => {
                    self.absorb(&report);
                    match report {
                        Report::Started => return Ok(()),
                        Report::Failed(failure) => return Err(failure.into_error()),
                        Report::Stopped { .. } => {
                            return Err(Error::Unanswered("starting"));
                        }
                        _ => {}
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    self.state = State::Failed;
                    return Err(Error::Unanswered("starting"));
                }
                // The reader thread is gone, which means the helper's output
                // closed: it exited without ever saying it had started.
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    self.state = State::Failed;
                    return Err(match self.failure.take() {
                        Some(words) => Error::Helper(words),
                        None => Error::Unanswered("starting"),
                    });
                }
            }
        }
    }

    /// Stops writing frames and stops the clock. Ignored unless the take is
    /// running.
    pub fn pause(&mut self) -> Result<()> {
        if self.state != State::Running {
            return Ok(());
        }
        self.say(Word::Pause)?;
        if let Ok(mut path) = self.path.lock() {
            path.pause();
        }
        self.state = State::Paused;
        Ok(())
    }

    /// Writes frames again. Ignored unless the take is paused.
    pub fn resume(&mut self) -> Result<()> {
        if self.state != State::Paused {
            return Ok(());
        }
        self.say(Word::Resume)?;
        if let Ok(mut path) = self.path.lock() {
            path.resume();
        }
        self.state = State::Running;
        Ok(())
    }

    /// Asks the helper to stop, waits for the file to be finalised, writes
    /// the pointer's path and the manifest, and hands back the bundle.
    ///
    /// A take with no picture is discarded rather than returned - a helper
    /// that failed before its first frame leaves nothing to import - and the
    /// failure that caused it is what comes back.
    pub fn finish(mut self) -> Result<Bundle> {
        let duration_ms = self.elapsed_ms();
        self.state = State::Stopping;
        // A broken pipe here means the helper has already gone, which is
        // what a stop asks for: wait for the reader either way.
        self.say(Word::Stop).ok();
        self.wait_for_stop();
        self.stop_following();

        // Copied out rather than taken: the reader thread may still be
        // holding the lock when the helper's pipe closes a moment later,
        // and a path worth keeping is not worth a race over.
        let samples = Telemetry {
            version: cursor::VERSION,
            samples: self
                .path
                .lock()
                .map(|path| path.samples_so_far())
                .unwrap_or_default(),
        };

        let mut bundle = std::mem::replace(
            &mut self.bundle,
            Bundle {
                folder: PathBuf::new(),
                manifest: bundle::Manifest::new(
                    &Source::Screen {
                        id: 0,
                        area: crate::source::Area::sized(1.0, 1.0),
                    },
                    1,
                ),
            },
        );

        if !bundle.has_picture() {
            bundle.discard().ok();
            return Err(match self.failure.take() {
                Some(words) => Error::Helper(words),
                None => Error::Unanswered("stopping"),
            });
        }

        bundle.manifest.duration_ms = duration_ms;
        bundle.manifest.pointer_samples = samples.samples.len();
        bundle.manifest.system_audio = kept(&bundle.folder, bundle::SYSTEM_AUDIO);
        bundle.manifest.microphone = kept(&bundle.folder, bundle::MICROPHONE);
        samples.write_beside(&bundle.video())?;
        bundle.write_manifest()?;
        self.state = State::Stopped;
        Ok(bundle)
    }

    /// Stops the take and throws it away.
    pub fn cancel(mut self) {
        self.say(Word::Stop).ok();
        self.wait_for_stop();
        self.stop_following();
        self.bundle.discard().ok();
        self.state = State::Stopped;
    }

    /// Spawns a helper with its configuration as one argument.
    fn spawn(helper: &Path, json: &str, cwd: &Path) -> Result<Child> {
        log::info!("capture helper {} starting", helper.display());
        Command::new(helper)
            .arg(json)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(Error::Io)
    }

    /// Starts the threads that read the screen helper's two streams.
    fn read_helper(&mut self, sender: mpsc::Sender<Report>) {
        let streams: [Option<Box<dyn BufRead + Send>>; 2] = [
            self.screen
                .stdout
                .take()
                .map(|stream| Box::new(BufReader::new(stream)) as Box<dyn BufRead + Send>),
            self.screen
                .stderr
                .take()
                .map(|stream| Box::new(BufReader::new(stream)) as Box<dyn BufRead + Send>),
        ];
        for stream in streams.into_iter().flatten() {
            let sender = sender.clone();
            std::thread::spawn(move || {
                for line in stream.lines().map_while(std::result::Result::ok) {
                    let report = Report::read(&line);
                    if let Report::Said(words) = &report {
                        if !words.is_empty() {
                            log::debug!("capture helper: {words}");
                        }
                        continue;
                    }
                    log::debug!("capture helper: {report:?}");
                    if sender.send(report).is_err() {
                        // Nobody is listening: the take is over.
                        return;
                    }
                }
            });
        }
    }

    /// Starts the pointer monitor and the sampling timer. A take whose
    /// pointer cannot be followed - no helper on this machine, or a platform
    /// with no pointer to ask about - is still a take: the failure is logged
    /// and the picture is recorded without a path.
    fn start_following(&mut self) {
        match Helper::Pointer.locate() {
            Ok(helper) => match Session::spawn(&helper, "", &self.bundle.folder) {
                Ok(mut child) => {
                    if let Some(stream) = child.stdout.take() {
                        let path = Arc::clone(&self.path);
                        std::thread::spawn(move || {
                            let stream = BufReader::new(stream);
                            for line in stream.lines().map_while(std::result::Result::ok) {
                                if let Ok(mut path) = path.lock() {
                                    path.hear(&line);
                                }
                            }
                        });
                    }
                    self.pointer = Some(child);
                }
                Err(error) => log::warn!("the pointer monitor would not start: {error}"),
            },
            Err(error) => log::warn!("no pointer monitor: {error}"),
        }

        if cursor::position().is_none() {
            log::warn!("this platform will not say where the pointer is; no path will be recorded");
            return;
        }
        self.sampling.store(true, Ordering::Release);
        let path = Arc::clone(&self.path);
        let sampling = Arc::clone(&self.sampling);
        std::thread::spawn(move || {
            while sampling.load(Ordering::Acquire) {
                match path.lock() {
                    Ok(mut path) => path.sample(),
                    // The take is being finished; the lock's owner has the
                    // path and this thread has nothing left to add.
                    Err(_) => return,
                }
                std::thread::sleep(Duration::from_millis(cursor::INTERVAL_MS));
            }
        });
    }

    /// Stops the pointer monitor and the sampling timer.
    fn stop_following(&mut self) {
        self.sampling.store(false, Ordering::Release);
        if let Some(mut child) = self.pointer.take() {
            // The monitor takes no words; closing its input and killing it
            // is how it ends. It holds an event tap, so leaving it running
            // would keep watching the keyboard after the take.
            drop(child.stdin.take());
            child.kill().ok();
            child.wait().ok();
        }
    }

    /// Passes a word to the screen helper.
    fn say(&mut self, word: Word) -> Result<()> {
        let Some(stdin) = self.screen.stdin.as_mut() else {
            return Err(Error::Unanswered("speaking to the helper"));
        };
        stdin.write_all(word.line().as_bytes())?;
        stdin.flush()?;
        Ok(())
    }

    /// Waits for the helper to finalise and exit, up to [`STOP_TIMEOUT`].
    /// A helper that will not go is killed: an unfinalised file is better
    /// than an editor that will not quit.
    fn wait_for_stop(&mut self) {
        let deadline = Instant::now() + STOP_TIMEOUT;
        loop {
            while let Ok(report) = self.reports.try_recv() {
                self.absorb(&report);
            }
            match self.screen.try_wait() {
                Ok(Some(status)) => {
                    if !status.success() {
                        log::warn!("the capture helper exited with {status}");
                    }
                    // Whatever it said on the way out.
                    while let Ok(report) = self.reports.try_recv() {
                        self.absorb(&report);
                    }
                    return;
                }
                Ok(None) => {}
                Err(error) => {
                    log::warn!("the capture helper could not be waited for: {error}");
                    return;
                }
            }
            if Instant::now() >= deadline {
                log::warn!("the capture helper did not stop; killing it");
                self.screen.kill().ok();
                self.screen.wait().ok();
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Brings the take's state up to date from one report.
    fn absorb(&mut self, report: &Report) {
        match report {
            Report::Started => {
                if self.state == State::Starting {
                    // The clock starts with the first frame, not with the
                    // spawn: see the module's documentation.
                    if let Ok(mut path) = self.path.lock() {
                        *path = Recorder::new(self.bundle.manifest.area);
                    }
                    self.state = State::Running;
                }
            }
            Report::Paused => self.state = State::Paused,
            Report::Resumed => self.state = State::Running,
            Report::Stopped { .. } => {
                if self.state != State::Failed {
                    self.state = State::Stopped;
                }
            }
            Report::Failed(failure) => {
                self.failure = Some(failure.clone().into_error().to_string());
                self.state = State::Failed;
            }
            Report::SystemAudio { .. } | Report::Microphone { .. } | Report::Said(_) => {}
        }
    }
}

impl Drop for Session {
    /// A take that is dropped without being finished is stopped anyway: the
    /// helpers hold the display and the event tap, and leaving them running
    /// after the editor has forgotten about them is the worst outcome
    /// available.
    fn drop(&mut self) {
        self.sampling.store(false, Ordering::Release);
        if matches!(self.state, State::Starting | State::Running | State::Paused) {
            self.say(Word::Stop).ok();
            self.wait_for_stop();
        }
        self.stop_following();
    }
}

/// The file name, if a file of that name was written with something in it.
/// The helpers are asked for sound and may not manage it - no audio device,
/// a permission refused mid-take - and the manifest should say what is
/// there rather than what was asked for.
fn kept(folder: &Path, name: &str) -> Option<String> {
    std::fs::metadata(folder.join(name))
        .ok()
        .filter(|file| file.len() > 0)
        .map(|_| name.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_take_follows_the_pointer_and_keeps_the_desktop_s_sound() {
        let settings = Settings::default();
        assert_eq!(settings.fps, 60);
        assert!(settings.system_audio);
        assert!(settings.follow_pointer);
        assert!(!settings.microphone);
    }

    #[test]
    fn sound_is_only_recorded_as_kept_when_it_was_written() {
        let dir = std::env::temp_dir().join(format!("concat-capture-kept-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a temporary folder");
        assert_eq!(kept(&dir, bundle::SYSTEM_AUDIO), None);
        std::fs::write(dir.join(bundle::SYSTEM_AUDIO), []).expect("an empty file");
        assert_eq!(kept(&dir, bundle::SYSTEM_AUDIO), None);
        std::fs::write(dir.join(bundle::SYSTEM_AUDIO), [0u8; 4]).expect("a file");
        assert_eq!(
            kept(&dir, bundle::SYSTEM_AUDIO),
            Some(bundle::SYSTEM_AUDIO.to_owned())
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_take_whose_helper_is_missing_leaves_no_folder() {
        unsafe { std::env::set_var("CONCAT_CAPTURE_HELPERS", "/concat-capture-nowhere") };
        let root = std::env::temp_dir().join(format!(
            "concat-capture-nohelper-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&root).expect("a temporary folder");
        let source = Source::Screen {
            id: 1,
            area: crate::source::Area::sized(1920.0, 1080.0),
        };
        let failure = Session::start(&root, source, &Settings::default()).err();
        assert!(
            matches!(failure, Some(Error::NoHelper { .. })),
            "{failure:?}"
        );
        assert_eq!(
            std::fs::read_dir(&root).into_iter().flatten().count(),
            0,
            "a take that never started left a folder behind"
        );
        std::fs::remove_dir_all(&root).ok();
    }
}
