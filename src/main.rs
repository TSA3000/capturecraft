//! CaptureCraft CLI.

use anyhow::{Context, Result, bail};
use capturecraft::Region;
use capturecraft::config::{Config, SelectMode, config_path};
use capturecraft::tray::{self, TrayOptions};
use std::path::PathBuf;

fn usage() -> String {
    format!(
        "\
usage: capturecraft [MODE] [OPTIONS] [output.png]

modes (default: `selection` from the config file):
  --select           freeze all screens; drag a region or click a window
                     arrows = nudge 1 px (Shift 10), Esc = cancel
  --window, -w       freeze all screens; click a window
  --tray, -t         run in the background with a tray icon and global hotkey
  --full, -f         capture the whole primary screen, no overlay
  --region, -r R     capture R = x,y,width,height from the primary screen

options (override {}):
  --hotkey KEY       tray hotkey, e.g. ctrl+shift+s
  --out-dir DIR      tray output folder
  --no-clipboard     don't copy the result to the clipboard
  --delay-ms MS      wait before capturing
  --list-windows     print monitors and snap targets, then exit

Without output.png, captures are saved to the configured output folder.",
        config_path().display()
    )
}

enum Mode {
    Interactive(SelectMode),
    Tray,
    Full,
    Region(Region),
}

fn main() -> Result<()> {
    let mut mode: Option<Mode> = None;
    let mut no_clipboard = false;
    let mut out: Option<PathBuf> = None;
    let mut hotkey: Option<String> = None;
    let mut out_dir: Option<PathBuf> = None;
    let mut delay_ms: u64 = 0;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-t" | "--tray" => mode = Some(Mode::Tray),
            "--select" => mode = Some(Mode::Interactive(SelectMode::Region)),
            "-w" | "--window" => mode = Some(Mode::Interactive(SelectMode::Window)),
            "-f" | "--full" => mode = Some(Mode::Full),
            "-r" | "--region" => {
                let value = args.next().context("--region needs a value")?;
                mode = Some(Mode::Region(
                    value.parse::<Region>().map_err(anyhow::Error::msg)?,
                ));
            }
            "--hotkey" => hotkey = Some(args.next().context("--hotkey needs a value")?),
            "--out-dir" => out_dir = Some(args.next().context("--out-dir needs a value")?.into()),
            "--no-clipboard" => no_clipboard = true,
            "--list-windows" => return capturecraft::overlay::debug_list_windows(),
            "--delay-ms" => {
                let value = args.next().context("--delay-ms needs a value")?;
                delay_ms = value.parse().context("--delay-ms must be a number")?;
            }
            "-h" | "--help" => {
                println!("{}", usage());
                return Ok(());
            }
            other if other.starts_with('-') => bail!("unknown option '{other}'\n\n{}", usage()),
            other => out = Some(PathBuf::from(other)),
        }
    }

    let mut config = Config::load_or_create()?;
    if let Some(h) = hotkey {
        config.hotkeys = vec![h];
    }
    if let Some(d) = out_dir {
        config.output_dir = d;
    }
    if no_clipboard {
        config.clipboard = false;
    }

    let mode = mode.unwrap_or(match config.selection {
        SelectMode::Fullscreen => Mode::Full,
        m => Mode::Interactive(m),
    });
    if let Mode::Tray = mode {
        return tray::run(TrayOptions::from(config));
    }
    let delay = std::time::Duration::from_millis(delay_ms);

    // Without an explicit file name: capture, save to the output folder and
    // copy to the clipboard (same path the window and tray use).
    let select = match mode {
        Mode::Interactive(m) => Some(m),
        Mode::Full => Some(SelectMode::Fullscreen),
        _ => None,
    };
    if let (None, Some(m)) = (&out, select) {
        return match capturecraft::run_capture(&config, m, delay)? {
            Some(path) => {
                println!("saved {}", path.display());
                Ok(())
            }
            None => {
                println!("cancelled");
                Ok(())
            }
        };
    }

    std::thread::sleep(delay);
    let img = match mode {
        Mode::Tray => unreachable!(),
        Mode::Interactive(m) => match capturecraft::capture_interactive(m, config.overlay.clone())?
        {
            Some(img) => img,
            None => {
                println!("cancelled");
                return Ok(());
            }
        },
        Mode::Full => capturecraft::capture_primary()?,
        Mode::Region(r) => capturecraft::capture_primary_region(r)?,
    };

    let out = match out {
        Some(path) => {
            img.save(&path)
                .with_context(|| format!("failed to save {}", path.display()))?;
            path
        }
        None => tray::save_capture(&img, &config.output_dir)?,
    };
    println!("saved {} ({}x{})", out.display(), img.width(), img.height());

    if config.clipboard {
        match capturecraft::copy_to_clipboard(&img) {
            Ok(()) => println!("copied to clipboard"),
            Err(e) => eprintln!("warning: {e:#}"),
        }
    }
    Ok(())
}
