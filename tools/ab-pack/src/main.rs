//! `ab-pack`: April Blink asset packs.
//!
//! ```text
//! ab-pack create assets -o game.abp          # pack a directory
//! ab-pack append target/release/game assets -o dist/game   # single-file game
//! ab-pack list game.abp                       # contents and sizes
//! ab-pack verify game.abp                     # read and check every file
//! ab-pack extract game.abp -o unpacked/       # get the files back
//! ```

use ab_vfs::{append_to_executable, Compression, Pack, PackStats, PackWriter};
use anyhow::{bail, Context as _};
use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};

#[derive(Parser, Debug)]
#[command(
    name = "ab-pack",
    version,
    about = "Create and inspect April Blink asset packs (.abp)"
)]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Packs every file of a directory (hidden ones skipped).
    Create {
        input: PathBuf,
        #[arg(short, long)]
        output: PathBuf,
    },
    /// Writes a program with the packed directory appended: a single-file game.
    Append {
        executable: PathBuf,
        input: PathBuf,
        #[arg(short, long)]
        output: PathBuf,
    },
    /// Lists the files of a pack.
    List { pack: PathBuf },
    /// Reads every file and checks its checksum.
    Verify { pack: PathBuf },
    /// Writes the files of a pack to a directory.
    Extract {
        pack: PathBuf,
        #[arg(short, long)]
        output: PathBuf,
    },
}

fn main() -> anyhow::Result<()> {
    match Args::parse().command {
        Command::Create { input, output } => {
            let writer = collect(&input)?;
            let stats = writer
                .write_file(&output)
                .with_context(|| format!("writing {}", output.display()))?;
            report(&output, &stats);
        }
        Command::Append {
            executable,
            input,
            output,
        } => {
            if executable == output {
                bail!("the output must differ from the executable");
            }
            let writer = collect(&input)?;
            let stats = append_to_executable(&executable, &writer, &output)
                .with_context(|| format!("writing {}", output.display()))?;
            report(&output, &stats);
        }
        Command::List { pack } => {
            let pack = open(&pack)?;
            for entry in pack.entries() {
                let mode = match entry.compression {
                    Compression::Lz4 => "lz4",
                    Compression::None => "   ",
                };
                println!(
                    "{:>10}  {:>10}  {mode}  {}",
                    entry.size, entry.stored_size, entry.path
                );
            }
            println!("{} files", pack.len());
        }
        Command::Verify { pack: path } => {
            let pack = open(&path)?;
            pack.verify()
                .with_context(|| format!("{} is damaged", path.display()))?;
            println!("{}: {} files OK", path.display(), pack.len());
        }
        Command::Extract { pack, output } => {
            let pack = open(&pack)?;
            for entry in pack.entries() {
                // Never write outside `output` (`..`, absolute paths, Windows drives).
                let confined = Path::new(&entry.path)
                    .components()
                    .all(|c| matches!(c, std::path::Component::Normal(_)));
                if !confined {
                    bail!(
                        "refusing to extract `{}`: outside the output folder",
                        entry.path
                    );
                }
                let target = output.join(&entry.path);
                if let Some(parent) = target.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&target, pack.read(&entry.path)?)?;
            }
            println!("{} files written to {}", pack.len(), output.display());
        }
    }
    Ok(())
}

fn collect(input: &Path) -> anyhow::Result<PackWriter> {
    if !input.is_dir() {
        bail!("{} is not a directory", input.display());
    }
    let mut writer = PackWriter::new();
    writer
        .add_directory(input)
        .with_context(|| format!("reading {}", input.display()))?;
    Ok(writer)
}

fn open(path: &Path) -> anyhow::Result<Pack> {
    Pack::open(path).with_context(|| format!("opening {}", path.display()))
}

fn report(output: &Path, stats: &PackStats) {
    let ratio = if stats.original_bytes > 0 {
        stats.pack_bytes as f64 / stats.original_bytes as f64 * 100.0
    } else {
        100.0
    };
    println!(
        "{}: {} files ({} compressed), {} → {} bytes ({ratio:.0}%)",
        output.display(),
        stats.files,
        stats.compressed_files,
        stats.original_bytes,
        stats.pack_bytes
    );
}
