// SPDX-License-Identifier: AGPL-3.0-or-later
// SPDX-FileCopyrightText: 2026 Jareer Abdullah and Concat contributors

//! Imports a recorded take into a fresh project and prints what it became.
//!
//! The other half of `concat-capture`'s `take` example: that one records a
//! bundle, this one opens it.
//!
//! ```text
//! cargo run -p concat-host --example import_take -- "/tmp/takes/2026-10-22 18-04-11"
//! ```

use std::path::PathBuf;

use concat_capture::Bundle;
use concat_host::takes::{self, Zooming};
use concat_project::editor::Editor;

fn main() {
    let Some(folder) = std::env::args().nth(1).map(PathBuf::from) else {
        eprintln!("which take? pass the folder");
        std::process::exit(2);
    };
    let Some(bundle) = Bundle::open(&folder) else {
        eprintln!("{} is not a take", folder.display());
        std::process::exit(1);
    };
    println!(
        "take: {} - {:.1}s, {} pointer samples",
        bundle.manifest.source,
        bundle.manifest.duration_ms / 1000.0,
        bundle.manifest.pointer_samples
    );

    let mut editor = Editor::new();
    let imported = match takes::import(&mut editor, &bundle, 0.0, &Zooming::default()) {
        Ok(imported) => imported,
        Err(error) => {
            eprintln!("could not import: {error}");
            std::process::exit(1);
        }
    };

    let timeline = editor.project().active();
    println!(
        "frame {}x{} at {}/{}",
        timeline.video.width,
        timeline.video.height,
        timeline.video.rate_num,
        timeline.video.rate_den
    );
    for clip in &timeline.clips {
        let lane = timeline
            .tracks
            .iter()
            .position(|track| track.id == clip.track_id)
            .unwrap_or_default();
        println!(
            "  lane {lane} {:>8}  {:.2}s..{:.2}s  {}",
            format!("{:?}", clip.kind),
            clip.start,
            clip.start + clip.duration,
            clip.name
        );
    }
    println!("{} camera moves suggested:", imported.moves);
    let picture = timeline
        .clips
        .iter()
        .find(|clip| clip.id == imported.clip_id)
        .expect("the picture's clip");
    for one in &picture.camera {
        println!(
            "  {:>6}  {:.2}s..{:.2}s  depth {:.1}  onto ({:.3}, {:.3})  {:?}",
            one.id, one.start, one.end, one.depth, one.focus_x, one.focus_y, one.mode
        );
    }

    // And prove it renders: the placement at the middle of the first move.
    if let Some(one) = picture.camera.first() {
        let camera = concat_project::camera::engine(&picture.camera).expect("a camera");
        let middle = concat_core::Rational::approximate((one.start + one.end) / 2.0)
            .unwrap_or(concat_core::Rational::ZERO);
        let framing = camera.at(middle);
        println!(
            "at {:.2}s the frame is {:.2}x, centred on ({:.3}, {:.3})",
            middle.as_f64(),
            framing.zoom,
            framing.focus.x,
            framing.focus.y
        );
    }
}
