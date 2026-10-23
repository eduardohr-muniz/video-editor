// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Jareer Abdullah and Concat contributors

//! Records a few seconds of the main screen and prints what it made.
//!
//! The crate's own end-to-end check: everything but the editor. Point
//! `CONCAT_CAPTURE_HELPERS` at a folder holding the capture helpers and run
//!
//! ```text
//! cargo run -p concat-capture --example take -- /tmp/takes 4
//! ```
//!
//! The first argument is where to put the take, the second how many seconds
//! to record. It is an example rather than a test because it needs a screen,
//! a granted permission and a few seconds of wall clock, none of which
//! belong in `cargo test`.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use concat_capture::session::{Session, Settings, State};

fn main() {
    let mut args = std::env::args().skip(1);
    let folder = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("concat-takes"));
    let seconds: f64 = args
        .next()
        .and_then(|word| word.parse().ok())
        .unwrap_or(4.0);

    std::fs::create_dir_all(&folder).expect("somewhere to put the take");

    let Some(screen) = concat_capture::main_screen() else {
        eprintln!("no screen to record");
        std::process::exit(1);
    };
    let (width, height) = screen.pixels();
    println!(
        "screen {} - {}x{} points, {width}x{height} pixels",
        screen.id, screen.area.width, screen.area.height
    );

    let settings = Settings {
        // The desktop's sound needs no device and no permission prompt,
        // and proves the second file is written.
        system_audio: true,
        microphone: false,
        ..Settings::default()
    };

    let mut session = match Session::start(&folder, screen.whole(), &settings) {
        Ok(session) => session,
        Err(error) => {
            eprintln!("could not start: {error}");
            std::process::exit(1);
        }
    };
    println!("spawned; waiting for the first frame");
    if let Err(error) = session.wait_until_running() {
        eprintln!("never started: {error}");
        std::process::exit(1);
    }
    println!("recording into {}", session.folder().display());

    let until = Instant::now() + Duration::from_secs_f64(seconds);
    while Instant::now() < until {
        for report in session.poll() {
            println!("  {report:?}");
        }
        if session.state() == State::Failed {
            eprintln!("the take failed partway through");
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    println!(
        "{:.1}s recorded, {} pointer samples",
        session.elapsed_ms() / 1000.0,
        session.pointer_samples()
    );

    match session.finish() {
        Ok(bundle) => {
            println!("bundle at {}", bundle.folder.display());
            println!("  manifest {:#?}", bundle.manifest);
            for (what, path) in [
                ("picture", Some(bundle.video())),
                ("desktop sound", bundle.system_audio()),
                ("microphone", bundle.microphone()),
                ("pointer", Some(bundle.pointer())),
            ] {
                match path {
                    Some(path) => {
                        let size = std::fs::metadata(&path).map(|file| file.len()).unwrap_or(0);
                        println!("  {what}: {} ({size} bytes)", path.display());
                    }
                    None => println!("  {what}: none"),
                }
            }
        }
        Err(error) => {
            eprintln!("could not finish: {error}");
            std::process::exit(1);
        }
    }
}
