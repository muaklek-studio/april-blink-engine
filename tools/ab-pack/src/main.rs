//! `ab-pack`: April Blink asset chunks and packs.
//!
//! ```text
//! ab-pack create assets -o dist/assets       # chunks named by hash + index.abm
//! ab-pack append target/release/game assets -o dist/game   # single-file game
//! ab-pack list dist/assets                    # contents and sizes
//! ab-pack verify dist/assets                  # read and check every file
//! ab-pack extract dist/assets -o unpacked/    # get the files back
//! ```
//!
//! `list`, `verify` and `extract` take a chunk folder (or its `index.abm`), a single chunk,
//! or a single-file game.

use ab_vfs::{
    append_to_executable, is_chunked, ChunkedSource, Compression, FileSource, Pack, PackStats,
    PackWriter,
};
use anyhow::{bail, Context as _};
use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};

#[derive(Parser, Debug)]
#[command(
    name = "ab-pack",
    version,
    about = "Create and inspect April Blink asset chunks and packs (.abp)"
)]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Packs a directory (hidden files skipped) as chunks (one per top-level folder, named by hash) and their index
    /// in `output`. Unchanged chunks already there are kept: the written ones and the index
    /// are the patch.
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
            let stats = collect(&input)?
                .write_chunks(&output)
                .with_context(|| format!("writing {}", output.display()))?;
            report(&output, &stats.totals);
            println!(
                "{} chunks: {} written, {} unchanged, {} stale removed",
                stats.chunks,
                stats.written.len(),
                stats.chunks - stats.written.len(),
                stats.removed
            );
            for name in &stats.written {
                println!("  {name}");
            }
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
        Command::List { pack } if chunk_folder(&pack).is_some() => {
            let chunks = open_chunks(&pack)?;
            let manifest = chunks.manifest();
            println!("index {:016x}", manifest.id());
            for (index, chunk) in manifest.chunks().iter().enumerate() {
                println!("{}  {:>10}  {}/", chunk.file_name(), chunk.size, chunk.name);
                let pack = chunks.chunk(index)?;
                for entry in pack.entries() {
                    println!("    {:>10}  {}", entry.size, entry.path);
                }
            }
            println!(
                "{} files in {} chunks",
                manifest.file_count(),
                manifest.chunks().len()
            );
        }
        Command::Verify { pack: path } if chunk_folder(&path).is_some() => {
            let chunks = open_chunks(&path)?;
            chunks
                .verify()
                .with_context(|| format!("{} is damaged", path.display()))?;
            println!(
                "{}: {} files in {} chunks OK",
                path.display(),
                chunks.manifest().file_count(),
                chunks.manifest().chunks().len()
            );
        }
        Command::Extract { pack, output } if chunk_folder(&pack).is_some() => {
            let chunks = open_chunks(&pack)?;
            let files = chunks.list();
            for path in &files {
                write_confined(&output, path, chunks.read(path)?)?;
            }
            println!("{} files written to {}", files.len(), output.display());
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
                write_confined(&output, &entry.path, pack.read(&entry.path)?)?;
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

/// Writes `bytes` to `output/path`, never outside `output` (`..`, absolute paths, Windows
/// drives).
fn write_confined(output: &Path, path: &str, bytes: Vec<u8>) -> anyhow::Result<()> {
    let confined = Path::new(path)
        .components()
        .all(|c| matches!(c, std::path::Component::Normal(_)));
    if !confined {
        bail!("refusing to extract `{path}`: outside the output folder");
    }
    let target = output.join(path);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&target, bytes)?;
    Ok(())
}

/// The chunk folder `path` names: the folder, or its index.
fn chunk_folder(path: &Path) -> Option<PathBuf> {
    let dir = if path.is_dir() { path } else { path.parent()? };
    let index = path.is_dir() || path.file_name() == Some(ab_vfs::MANIFEST_FILE.as_ref());
    (index && is_chunked(dir)).then(|| dir.to_path_buf())
}

fn open_chunks(path: &Path) -> anyhow::Result<ChunkedSource> {
    let dir = chunk_folder(path).with_context(|| format!("{} has no index", path.display()))?;
    ChunkedSource::open(&dir).with_context(|| format!("opening {}", dir.display()))
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
