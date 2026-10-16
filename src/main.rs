//! CaptureCraft CLI.

use anyhow::{Context, Result, bail};
use capturecraft::Region;
use capturecraft::tray::{self, DEFAULT_HOTKEY, TrayOptions};
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

options:
  --hotkey KEY       tray hotkey (default: {DEFAULT_HOTKEY}), e.g. ctrl+shift+s
  --out-dir DIR      tray output folder (default: {})
  --no-clipboard     don't copy the result to the clipboard",
        tray::default_output_dir().display()
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
    let mut clipboard = true;
    let mut out: Option<PathBuf> = None;
    let mut hotkey = DEFAULT_HOTKEY.to_string();
    let mut out_dir = tray::default_output_dir();

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-t" | "--tray" => mode = Mode::Tray,
            "-f" | "--full" => mode = Mode::Full,
            "-r" | "--region" => {
                let value = args.next().context("--region needs a value")?;
                mode = Mode::Region(value.parse::<Region>().map_err(anyhow::Error::msg)?);
            }
            "--hotkey" => hotkey = args.next().context("--hotkey needs a value")?,
            "--out-dir" => out_dir = args.next().context("--out-dir needs a value")?.into(),
            "--no-clipboard" => clipboard = false,
            "-h" | "--help" => {
                println!("{}", usage());
                return Ok(());
            }
            other if other.starts_with('-') => bail!("unknown option '{other}'\n\n{}", usage()),
            other => out = Some(PathBuf::from(other)),
        }
    }

    let img = match mode {
        Mode::Tray => {
            return tray::run(TrayOptions {
                hotkey,
                output_dir: out_dir,
                clipboard,
            });
        }
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

    if clipboard {
        match capturecraft::copy_to_clipboard(&img) {
            Ok(()) => println!("copied to clipboard"),
            Err(e) => eprintln!("warning: {e:#}"),
        }
    }
    Ok(())
}
