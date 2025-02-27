//! A simple parallel inode caching tool

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]
#![warn(
    clippy::alloc_instead_of_core,
    clippy::std_instead_of_alloc,
    clippy::std_instead_of_core
)]
#![warn(
    missing_docs,
    clippy::missing_docs_in_private_items,
    rustdoc::broken_intra_doc_links
)]

extern crate alloc;

use std::{io::Write, path::PathBuf, process::ExitCode};

mod worker;

use clap::{Parser, ValueHint};

#[derive(clap::Parser)]
#[clap(author = "ultrabear <bearodark@gmail.com>", version)]
/// A simple cli to load the metadata of given mountpoints into ram by reading them
struct Args {
    /// directories to traverse into
    #[arg(value_hint = ValueHint::FilePath)]
    dirs: Vec<PathBuf>,

    /// Read directory names from a newline delimited file or stdin (-)
    #[arg(long, value_hint = ValueHint::FilePath)]
    dir_file: Option<PathBuf>,
}

async fn tokio_main() {
    let start = std::time::Instant::now();

    let mut parse = Args::parse();

    let counts = worker::cache_dirs(parse.dirs).await;

    _ = writeln!(
        std::io::stdout().lock(),
        "Processed {counts} in {:?}",
        start.elapsed()
    );
}

fn main() -> ExitCode {
    let mut rt = tokio::runtime::Builder::new_multi_thread();
    rt.enable_all();
    // we mostly do io work, we want lots of syscalls on wait
    rt.worker_threads(64);

    match rt.build() {
        Ok(rt) => {
            rt.block_on(tokio_main());
            ExitCode::SUCCESS
        }
        Err(e) => {
            _ = writeln!(std::io::stderr().lock(), "Error initializing tokio: {e}");
            ExitCode::FAILURE
        }
    }
}
