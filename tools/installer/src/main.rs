//! Kingdom 3DS installer: pick your game folder and SD card, get the game on it.
//!
//! `kingdom-installer` opens the window. `kingdom-installer --cli --game DIR
//! --sd DIR [--cia] [--no-3dsx]` does the same without one.

#![cfg_attr(windows, windows_subsystem = "windows")]

mod assetripper;
mod gamefind;
mod gui;
mod pipeline;

use std::path::PathBuf;

pub type Log = dyn Fn(&str) + Send + Sync;

/// No console window flashing up for each helper program on Windows.
pub fn hide_window(cmd: &mut std::process::Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }
    #[cfg(not(windows))]
    let _ = cmd;
}

fn cli(args: &[String]) -> std::process::ExitCode {
    let value = |flag: &str| {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .map(PathBuf::from)
    };
    let (Some(game), Some(sd)) = (value("--game"), value("--sd")) else {
        eprintln!("usage: kingdom-installer --cli --game <game folder> --sd <SD card folder> [--cia] [--no-3dsx]");
        return std::process::ExitCode::FAILURE;
    };
    let o = pipeline::Options {
        game,
        sd,
        want_3dsx: !args.iter().any(|a| a == "--no-3dsx"),
        want_cia: args.iter().any(|a| a == "--cia"),
    };
    match pipeline::install(&o, &|l| println!("{l}")) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("Error: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn main() -> std::process::ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--cli") {
        return cli(&args);
    }
    match gui::run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            std::process::ExitCode::FAILURE
        }
    }
}
