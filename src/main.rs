//! CaptureCraft CLI.

use anyhow::{Context, Result, bail};
use capturecraft::Region;
use std::path::PathBuf;

const USAGE: &str = "\
usage: capturecraft [--full | --region x,y,width,height] [--no-clipboard] [output.png]

  (default)        freeze screen, drag to select (click = full screen, Esc = cancel)
  --full, -f       capture the whole primary screen, no overlay
  --region, -r     capture x,y,width,height from the primary screen, no overlay
  --no-clipboard   don't copy the result to the clipboard";

enum Mode {
    Interactive,
    Full,
    Region(Region),
}

fn main() -> Result<()> {
    let mut mode = Mode::Interactive;
    let mut clipboard = true;
    let mut out: Option<PathBuf> = None;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-f" | "--full" => mode = Mode::Full,
            "-r" | "--region" => {
                let value = args.next().context("--region needs a value")?;
                mode = Mode::Region(value.parse::<Region>().map_err(anyhow::Error::msg)?);
            }
            "--no-clipboard" => clipboard = false,
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(());
            }
            other if other.starts_with('-') => bail!("unknown option '{other}'\n\n{USAGE}"),
            other => out = Some(PathBuf::from(other)),
        }
    }

    let img = match mode {
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
