//! CaptureCraft CLI.

use anyhow::{Context, Result, bail};
use capturecraft::Region;
use capturecraft::config::{Config, config_path};
use capturecraft::tray::{self, TrayOptions};
use std::path::PathBuf;

fn usage() -> String {
    format!(
        "\
usage: capturecraft [MODE] [OPTIONS] [output.png]

modes:
  (default)          freeze all screens, drag to select
                     click = whole monitor, arrows = nudge 1 px (Shift 10), Esc = cancel
  --tray, -t         run in the background with a tray icon and global hotkey
  --full, -f         capture the whole primary screen, no overlay
  --region, -r R     capture R = x,y,width,height from the primary screen

options (override {}):
  --hotkey KEY       tray hotkey, e.g. ctrl+shift+s
  --out-dir DIR      tray output folder
  --no-clipboard     don't copy the result to the clipboard",
        config_path().display()
    )
}

enum Mode {
    Interactive,
    Tray,
    Full,
    Region(Region),
}

fn main() -> Result<()> {
    let mut mode = Mode::Interactive;
    let mut no_clipboard = false;
    let mut out: Option<PathBuf> = None;
    let mut hotkey: Option<String> = None;
    let mut out_dir: Option<PathBuf> = None;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-t" | "--tray" => mode = Mode::Tray,
            "-f" | "--full" => mode = Mode::Full,
            "-r" | "--region" => {
                let value = args.next().context("--region needs a value")?;
                mode = Mode::Region(value.parse::<Region>().map_err(anyhow::Error::msg)?);
            }
            "--hotkey" => hotkey = Some(args.next().context("--hotkey needs a value")?),
            "--out-dir" => out_dir = Some(args.next().context("--out-dir needs a value")?.into()),
            "--no-clipboard" => no_clipboard = true,
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

    let img = match mode {
        Mode::Tray => return tray::run(TrayOptions::from(config)),
        Mode::Interactive => match capturecraft::capture_interactive()? {
            Some(img) => img,
            None => {
                println!("cancelled");
                return Ok(());
            }
        },
        Mode::Full => capturecraft::capture_primary()?,
        Mode::Region(r) => capturecraft::capture_primary_region(r)?,
    };

    let out = out.unwrap_or_else(|| PathBuf::from("capture.png"));
    img.save(&out)
        .with_context(|| format!("failed to save {}", out.display()))?;
    println!("saved {} ({}x{})", out.display(), img.width(), img.height());

    if config.clipboard {
        match capturecraft::copy_to_clipboard(&img) {
            Ok(()) => println!("copied to clipboard"),
            Err(e) => eprintln!("warning: {e:#}"),
        }
    }
    Ok(())
}
