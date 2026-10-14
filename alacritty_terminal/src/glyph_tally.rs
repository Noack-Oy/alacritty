//! Tally of the non-ASCII characters this terminal puts on the grid.
//!
//! The question it answers is "which glyphs does the configured font have no
//! coverage for", and it answers it the long way round: record what is actually
//! rendered, and subtract the font's coverage afterwards, offline. Asking the
//! rasteriser directly would be more direct but is not possible from here --
//! crossfont resolves fallback in a private `face_for_glyph` and its
//! `RasterizedGlyph` carries no indication of which face produced it.
//!
//! Recording happens in `Term::input`, which runs once per character written to
//! the grid. That is deliberately *not* the render path: it counts occurrences
//! rather than frames, and it sees terminal UI drawn by the application --
//! status lines, curses chrome -- which never reaches a scrollback or a log and
//! so cannot be recovered from any corpus on disk.
//!
//! Disabled unless `ALACRITTY_GLYPH_TALLY` names a file. ASCII is never
//! recorded: every font that can serve as a terminal primary covers it, so it
//! would be the overwhelming majority of the data and none of the signal.
//!
//! Writes are appends of whole batches, never rewrites, because several
//! terminals run at once and a read-modify-write would lose the other windows'
//! counts. Repeated codepoints across batches are expected; whatever reads the
//! file sums them.

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::sync::{Mutex, OnceLock};

use log::{debug, warn};

/// Characters to accumulate before appending a batch.
///
/// Small on purpose. The interesting glyphs are rare -- a missing symbol in a
/// status line may appear a handful of times in a session -- so a large batch
/// would mean the only evidence worth having is also the evidence most likely
/// to be lost if the process is killed rather than closed.
const FLUSH_EVERY: u32 = 32;

struct Tally {
    counts: HashMap<char, u32>,
    pending: u32,
    path: String,
}

impl Tally {
    fn flush(&mut self) {
        if self.counts.is_empty() {
            return;
        }

        let mut batch = String::new();
        for (c, count) in &self.counts {
            batch.push_str(&format!("{:04X}\t{}\n", *c as u32, count));
        }

        match OpenOptions::new().create(true).append(true).open(&self.path) {
            Ok(mut file) => {
                if let Err(err) = file.write_all(batch.as_bytes()) {
                    warn!("Could not append to glyph tally {}: {}", self.path, err);
                } else {
                    debug!("Appended {} glyph tally entries", self.counts.len());
                }
            },
            Err(err) => warn!("Could not open glyph tally {}: {}", self.path, err),
        }

        // Cleared either way: a tally is not worth retrying into a path that is
        // not writable, and holding the counts would grow without bound.
        self.counts.clear();
        self.pending = 0;
    }
}

static TALLY: OnceLock<Option<Mutex<Tally>>> = OnceLock::new();

fn tally() -> Option<&'static Mutex<Tally>> {
    TALLY
        .get_or_init(|| {
            let path = std::env::var("ALACRITTY_GLYPH_TALLY").ok()?;
            debug!("Recording glyph tally to {}", path);
            Some(Mutex::new(Tally { counts: HashMap::new(), pending: 0, path }))
        })
        .as_ref()
}

/// Record one occurrence of a character written to the grid.
///
/// Callers filter ASCII out before calling, so the common path is one
/// comparison in `Term::input` and nothing else.
pub fn record(c: char) {
    let Some(tally) = tally() else { return };
    let Ok(mut tally) = tally.lock() else { return };

    *tally.counts.entry(c).or_insert(0) += 1;
    tally.pending += 1;

    if tally.pending >= FLUSH_EVERY {
        tally.flush();
    }
}

/// Append whatever has not been written yet.
///
/// Called on the way out of a clean exit. A terminal that is killed loses at
/// most `FLUSH_EVERY` characters' worth of counts.
pub fn flush() {
    let Some(tally) = tally() else { return };
    if let Ok(mut tally) = tally.lock() {
        tally.flush();
    }
}
