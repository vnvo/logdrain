//! `logdrain` CLI: read log lines from files or stdin, mine templates, and print
//! the resulting clusters as text, JSON, or CSV.

use std::fs::File;
use std::io::{self, BufRead, BufReader};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::Parser;
use logdrain::{builtin_masks, Mask, Miner};
use logdrain_cli::format::{self, Format, Sort};
use logdrain_cli::input;

#[derive(Parser, Debug)]
#[command(name = "logdrain", version, about = "Online log-template miner")]
struct Cli {
    /// Input files. Reads stdin if none are given.
    files: Vec<PathBuf>,

    /// JSON field to extract from each line (dot notation, e.g. `event.type`).
    /// When unset, each line is treated as raw text.
    #[arg(long)]
    key: Option<String>,

    /// Output format.
    #[arg(long, default_value = "text")]
    format: Format,

    /// Hide clusters smaller than N.
    #[arg(long, default_value_t = 0)]
    min_size: u64,

    /// Sort order.
    #[arg(long, default_value = "size")]
    sort: Sort,

    /// Similarity threshold in [0.0, 1.0].
    #[arg(long = "sim-th", default_value_t = 0.4)]
    sim_th: f64,

    /// Prefix-tree depth (>= 2).
    #[arg(long, default_value_t = 4)]
    depth: usize,

    /// Delimiter characters preserved as token boundaries, e.g. `/`.
    #[arg(long = "path-delimiters")]
    path_delimiters: Option<String>,

    /// Cluster on the first line only (keep the rest as a suffix).
    #[arg(long = "first-line-only")]
    first_line_only: bool,

    /// Comma-separated built-in masks: uuid,hex32,email,ipv4,jwt.
    #[arg(long)]
    masks: Option<String>,
}

fn build_masks(spec: &str) -> Result<Vec<Mask>, String> {
    spec.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|name| match name {
            "uuid" => Ok(builtin_masks::uuid()),
            "hex32" => Ok(builtin_masks::hex32()),
            "email" => Ok(builtin_masks::email()),
            "ipv4" => Ok(builtin_masks::ipv4()),
            "jwt" => Ok(builtin_masks::jwt()),
            other => Err(format!(
                "unknown mask '{other}' (known: uuid, hex32, email, ipv4, jwt)"
            )),
        })
        .collect()
}

fn build_miner(cli: &Cli) -> Result<Miner, String> {
    let mut b = Miner::builder()
        .sim_threshold(cli.sim_th)
        .depth(cli.depth)
        .first_line_only(cli.first_line_only);
    if let Some(pd) = &cli.path_delimiters {
        let chars: Vec<char> = pd.chars().collect();
        b = b.path_delimiters(&chars);
    }
    if let Some(spec) = &cli.masks {
        b = b.masks(build_masks(spec)?);
    }
    b.build().map_err(|e| e.to_string())
}

/// Feed one already-read line to the miner, applying `--key` extraction if set.
fn ingest_line(miner: &Miner, line: &str, key: Option<&str>) {
    match key {
        Some(k) => {
            if let Some(msg) = input::extract_field(line, k) {
                miner.add(&msg);
            }
        }
        None => {
            if !line.trim().is_empty() {
                miner.add(line);
            }
        }
    }
}

fn ingest_reader(miner: &Miner, reader: impl BufRead, key: Option<&str>) -> io::Result<()> {
    for line in reader.lines() {
        ingest_line(miner, &line?, key);
    }
    Ok(())
}

fn run(cli: Cli) -> Result<(), String> {
    let miner = build_miner(&cli)?;
    let key = cli.key.as_deref();

    if cli.files.is_empty() {
        let stdin = io::stdin();
        ingest_reader(&miner, stdin.lock(), key).map_err(|e| e.to_string())?;
    } else {
        for path in &cli.files {
            let f = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
            ingest_reader(&miner, BufReader::new(f), key).map_err(|e| e.to_string())?;
        }
    }

    let clusters = format::prepare(miner.clusters(), cli.min_size, cli.sort);
    print!("{}", format::render(&clusters, cli.format));
    Ok(())
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("logdrain: {e}");
            ExitCode::from(2)
        }
    }
}
