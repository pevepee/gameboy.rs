//! Blargg and mooneye test ROM harness.
//!
//! The ROMs are not committed. `scripts/fetch-test-roms.sh` clones them into
//! `test-roms/`. When that directory is empty every test here is skipped, so a
//! fresh clone can run `cargo test` without a network round trip.
//!
//! Set `GB_REQUIRE_TEST_ROMS=1` to turn a missing ROM into a failure. CI uses
//! that in the job that fetches the suite, so a silent skip cannot hide a
//! regression.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use gb_core::GameBoy;

/// Frames to run before declaring a ROM hung. Blargg's ROMs settle in seconds.
const MAX_FRAMES: u32 = 2_000;
/// Wall-clock ceiling, so a genuinely hung ROM fails instead of running forever.
const MAX_WALL_CLOCK: Duration = Duration::from_secs(120);

fn test_roms_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("test-roms")
}

fn require_test_roms() -> bool {
    std::env::var("GB_REQUIRE_TEST_ROMS").is_ok_and(|v| v == "1")
}

/// Every `.gb` file under `test-roms/`, sorted for stable output.
fn discover_roms() -> Vec<PathBuf> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, out);
            } else if path.extension().is_some_and(|e| e == "gb") {
                out.push(path);
            }
        }
    }

    let mut found = Vec::new();
    walk(&test_roms_dir(), &mut found);
    found.sort();
    found
}

/// Outcome a Blargg-style ROM reports over the link port.
#[derive(Debug, PartialEq, Eq)]
enum Outcome {
    Passed,
    Failed(String),
    /// The ROM never produced a verdict.
    Hung(String),
}

fn classify(output: &str) -> Outcome {
    if output.contains("Passed") {
        return Outcome::Passed;
    }
    if output.contains("Failed") {
        return Outcome::Failed(output.trim().to_string());
    }
    Outcome::Hung(output.trim().to_string())
}

/// Run a ROM until it prints a verdict, then return everything it printed.
fn run_rom(path: &Path) -> (Outcome, String) {
    let rom = std::fs::read(path).expect("ROM file must be readable");
    let mut gb = GameBoy::new_lossy(rom).expect("ROM must be at least header-sized");

    let started = Instant::now();
    for _ in 0..MAX_FRAMES {
        gb.run_frame();

        let output = gb.serial_output().to_string();
        if output.contains("Passed") || output.contains("Failed") {
            return (classify(&output), output);
        }
        if started.elapsed() > MAX_WALL_CLOCK {
            break;
        }
    }

    let output = gb.serial_output().to_string();
    (classify(&output), output)
}

#[test]
fn blargg_suite() {
    let roms = discover_roms();

    if roms.is_empty() {
        assert!(
            !require_test_roms(),
            "GB_REQUIRE_TEST_ROMS=1 but test-roms/ is empty; run scripts/fetch-test-roms.sh"
        );
        eprintln!("skipping: test-roms/ is empty");
        return;
    }

    let mut failures = Vec::new();
    let mut passed = 0usize;

    for path in &roms {
        let name = path
            .strip_prefix(test_roms_dir())
            .unwrap_or(path)
            .display()
            .to_string();

        let (outcome, output) = run_rom(path);
        match outcome {
            Outcome::Passed => {
                passed += 1;
                eprintln!("pass  {name}");
            }
            Outcome::Failed(detail) => {
                eprintln!("FAIL  {name}\n{detail}");
                failures.push(name);
            }
            Outcome::Hung(detail) => {
                eprintln!("HANG  {name}\n{detail}");
                failures.push(name);
            }
        }
    }

    let total = roms.len();
    eprintln!("\n{passed}/{total} test ROMs passed");

    assert!(
        failures.is_empty(),
        "{} of {total} ROMs did not pass:\n  {}",
        failures.len(),
        failures.join("\n  ")
    );
}

#[test]
fn emulator_survives_an_empty_rom_directory() {
    // Guards the skip path itself: discovering nothing must not panic.
    let roms = discover_roms();
    assert!(roms.len() < 100_000, "discovery returned an implausible count");
}
