use std::{
    io::Write,
    path::{Path, PathBuf},
    thread::sleep,
    time::Duration,
};

use clap::Parser;
use rayon::iter::{IntoParallelIterator, ParallelBridge, ParallelIterator};
use walkdir::WalkDir;


fn cache_dir(dir: impl AsRef<Path>) -> impl ParallelIterator<Item = walkdir::Error> {
    WalkDir::new(dir)
        .same_file_system(true)
        .into_iter()
        .par_bridge()
        .filter_map(|res| match res {
            Ok(entry) => entry.metadata().err(),
            Err(e) => Some(e),
        })
}

#[derive(clap::Parser)]
#[clap(author = "ultrabear <bearodark@gmail.com>", version = "1.0")]
/// A simple cli to load the metadata of given mountpoints into ram by reading them
struct Args {
    /// directories to traverse into
    #[arg(num_args = 1..)]
    dirs: Vec<PathBuf>,

    /// wait a number of seconds before starting
    #[arg(long)]
    wait: Option<u16>,
}

fn main() {
    let parse = Args::parse();

    if let Some(wait) = parse.wait {
        sleep(Duration::from_secs(wait.into()));
    }

    parse
        .dirs
        .into_par_iter()
        .flat_map(cache_dir)
        .for_each(|e| {
            _ = writeln!(std::io::stderr().lock(), "{e}");
        });
}
