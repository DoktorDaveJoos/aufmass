use anyhow::Result;
use clap::Parser;
use aufmass::{collect_inputs, convert, default_out, report, telemetry, update, Config};
use std::path::PathBuf;

/// Reads architect DWG/DXF plans that contain 3D solids and lists the walls,
/// slabs and roofs with their quantities.
#[derive(Parser)]
#[command(version)]
struct Args {
    /// DWG/DXF files or folders (searched recursively)
    #[arg(required_unless_present = "update")]
    inputs: Vec<PathBuf>,
    /// Output folder [default: "aufmass" next to the first input]
    #[arg(short, long)]
    out: Option<PathBuf>,
    /// Config file [default: ./aufmass.toml, else built-in defaults]
    #[arg(short, long)]
    config: Option<PathBuf>,
    /// Install the latest release from GitHub, for the CLI and the app
    #[arg(long, exclusive = true)]
    update: bool,
}

fn main() -> Result<()> {
    let _sentry = telemetry::init("cli");
    let args = Args::parse();
    if args.update {
        return self_update();
    }
    let cfg = Config::load(args.config.as_deref())?;
    let files = collect_inputs(&args.inputs)?;
    let out = args.out.unwrap_or_else(|| default_out(&args.inputs[0]));
    telemetry::set_folders(&args.inputs, &out);
    let conversion = convert(&files, &cfg);
    for f in &conversion.files {
        eprintln!("  {}", f.line());
    }
    let result = conversion.check().and_then(|()| conversion.write(&out));
    telemetry::conversion(&conversion.files, result.as_ref().err());
    result?;
    report::print_summary(&conversion.building);
    println!("\nGeschrieben nach {}: building.json, mengen.xlsx, mengen.x83 (GAEB), vorschau-*.svg", out.display());
    Ok(())
}

fn self_update() -> Result<()> {
    let Some(release) = update::check()? else {
        println!("Aufmaß {} ist aktuell.", update::VERSION);
        return Ok(());
    };
    println!("Lade Aufmaß {} …", release.version);
    let result = update::install(&release);
    if let Err(e) = &result {
        telemetry::update_failed(e);
    }
    result?;
    println!("Aufmaß {} installiert.", release.version);
    Ok(())
}
