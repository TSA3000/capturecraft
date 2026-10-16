//! `capturecraft [--region x,y,width,height] [output.png]`

use anyhow::{Context, Result, bail};
use capturecraft::Region;
use std::path::PathBuf;

const USAGE: &str = "usage: capturecraft [--region x,y,width,height] [output.png]";

fn main() -> Result<()> {
    let mut region: Option<Region> = None;
    let mut out: Option<PathBuf> = None;

    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-r" | "--region" => {
                let value = args.next().context("--region needs a value")?;
                region = Some(value.parse::<Region>().map_err(anyhow::Error::msg)?);
            }
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(());
            }
            other if other.starts_with('-') => bail!("unknown option '{other}'\n{USAGE}"),
            other => out = Some(PathBuf::from(other)),
        }
    }

    let out = out.unwrap_or_else(|| PathBuf::from("capture.png"));
    let img = match region {
        Some(r) => capturecraft::capture_primary_region(r)?,
        None => capturecraft::capture_primary()?,
    };
    img.save(&out)
        .with_context(|| format!("failed to save {}", out.display()))?;
    println!("saved {} ({}x{})", out.display(), img.width(), img.height());
    Ok(())
}
