//! Native frontend: a window, a keyboard map, and a frame pacer.
//!
//! The emulator core has no notion of wall-clock time. This binary is what turns
//! `run_frame` calls into something that plays at 59.7 Hz.

use std::process::ExitCode;
use std::time::{Duration, Instant};

use clap::Parser;
use gb_core::{Button, GameBoy, HEIGHT, WIDTH};
use minifb::{Key, Scale, Window, WindowOptions};

#[derive(Parser)]
#[command(name = "gameboy", about = "Run a Game Boy or Game Boy Color ROM")]
struct Args {
    /// Path to a `.gb` ROM file.
    rom: std::path::PathBuf,

    /// Integer window scale factor. Values other than a power of two fall back
    /// to 4x, because minifb only offers power-of-two scale variants.
    #[arg(long, default_value_t = 4)]
    scale: usize,

    /// Emulated frames per second target. Raise it to fast-forward.
    #[arg(long, default_value_t = 59.7275)]
    fps: f64,
}

fn main() -> ExitCode {
    let args = Args::parse();

    let rom = match std::fs::read(&args.rom) {
        Ok(bytes) => bytes,
        Err(err) => {
            eprintln!("cannot read {}: {err}", args.rom.display());
            return ExitCode::FAILURE;
        }
    };

    let mut gb = match GameBoy::new_lossy(rom) {
        Some(gb) => gb,
        None => {
            eprintln!("{} is too small to be a cartridge", args.rom.display());
            return ExitCode::FAILURE;
        }
    };

    println!("loaded: {}", gb.bus.cart.title());

    let options = WindowOptions {
        scale: scale_factor(args.scale),
        resize: false,
        ..WindowOptions::default()
    };

    // The window is created at the buffer's own size; `scale` makes minifb
    // present it larger without an intermediate buffer.
    let mut window = match Window::new("gameboy-rs", WIDTH, HEIGHT, options) {
        Ok(window) => window,
        Err(err) => {
            eprintln!("cannot open a window: {err}");
            return ExitCode::FAILURE;
        }
    };

    window.set_target_fps(0);

    let mut pixels = vec![0u32; WIDTH * HEIGHT];
    let frame_time = Duration::from_secs_f64(1.0 / args.fps.max(1.0));

    let mut next_frame = Instant::now();

    while window.is_open() && !window.is_key_down(Key::Escape) {
        sync_buttons(&mut gb, &window);

        gb.run_frame();

        for (index, &color) in gb.framebuffer_rgb().iter().enumerate() {
            pixels[index] = color;
        }

        if window.update_with_buffer(&pixels, WIDTH, HEIGHT).is_err() {
            break;
        }

        // Pace by absolute deadlines so a slow frame does not accumulate drift.
        next_frame += frame_time;
        let now = Instant::now();
        if next_frame > now {
            std::thread::sleep(next_frame - now);
        } else {
            next_frame = now;
        }
    }

    println!("serial output:\n{}", gb.serial_output());
    ExitCode::SUCCESS
}

/// minifb only has power-of-two scale variants, so the CLI value is a friendly
/// multiplier that has to be narrowed to one of them.
fn scale_factor(log2: usize) -> Scale {
    match log2 {
        1 => Scale::X1,
        2 => Scale::X2,
        8 => Scale::X8,
        16 => Scale::X16,
        32 => Scale::X32,
        _ => Scale::X4,
    }
}

/// Mirror the keyboard into the joypad. `press` only signals on a transition,
/// so re-asserting every frame is the intended usage.
fn sync_buttons(gb: &mut GameBoy, window: &Window) {
    const MAP: [(Key, Button); 8] = [
        (Key::Right, Button::Right),
        (Key::Left, Button::Left),
        (Key::Up, Button::Up),
        (Key::Down, Button::Down),
        (Key::Z, Button::A),
        (Key::X, Button::B),
        (Key::Enter, Button::Start),
        (Key::Backspace, Button::Select),
    ];

    for (key, button) in MAP {
        if window.is_key_down(key) {
            gb.press(button);
        } else {
            gb.release(button);
        }
    }
}
