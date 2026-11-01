use anyhow::Result;
use clap::Parser;
use aufmass::{collect_inputs, convert, default_out, report, Config};
use std::path::PathBuf;

/// Reads architect DWG/DXF plans that contain 3D solids and lists the walls,
/// slabs and roofs with their quantities.
#[derive(Parser)]
#[command(version)]
struct Args {
    /// DWG/DXF files or folders (searched recursively)
    #[arg(required = true)]
    inputs: Vec<PathBuf>,
    /// Output folder [default: "aufmass" next to the first input]
    #[arg(short, long)]
    out: Option<PathBuf>,
    /// Config file [default: ./aufmass.toml, else built-in defaults]
    #[arg(short, long)]
    config: Option<PathBuf>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let cfg = Config::load(args.config.as_deref())?;
    let files = collect_inputs(&args.inputs)?;
    let conversion = convert(&files, &cfg);
    for f in &conversion.files {
        eprintln!("  {}", f.line());
    }
    conversion.check()?;
    let out = args.out.unwrap_or_else(|| default_out(&args.inputs[0]));
    conversion.write(&out)?;
    report::print_summary(&conversion.building);
    println!("\nGeschrieben nach {}: building.json, mengen.xlsx, mengen.x83 (GAEB), vorschau-*.svg", out.display());
    Ok(())
}
