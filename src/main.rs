//! A simple parallel inode caching tool

#![forbid(unsafe_code)]
#![warn(clippy::pedantic)]
#![warn(clippy::alloc_instead_of_core, clippy::std_instead_of_alloc)]
#![warn(missing_docs/*, clippy::missing_docs_in_private_items*/)]

extern crate alloc;

use core::{
    fmt,
    future::Future,
    sync::atomic::{self, AtomicU64},
};

use alloc::sync::Arc;

use std::{
    fs::Metadata,
    io::{self, Write},
    os::unix::fs::MetadataExt,
    path::PathBuf,
    time::Duration,
};

use clap::Parser;
use crossbeam_utils::CachePadded;
use tokio::{sync::mpsc, task, time::sleep};
use tokio_util::task::TaskTracker;

#[derive(Clone)]
struct TaskSpawner(usize, TaskTracker);

impl TaskSpawner {
    async fn spawn<F: Future + Send + 'static>(&self, task: F) -> task::JoinHandle<F::Output>
    where
        F::Output: Send + 'static,
    {
        while self.1.len() > self.0 {
            sleep(Duration::from_micros(500)).await;
        }

        self.1.spawn(task)
    }
}

struct StatsTrackers {
    file: CachePadded<AtomicU64>,
    sym: CachePadded<AtomicU64>,
    dir: CachePadded<AtomicU64>,
}

impl StatsTrackers {
    const fn new() -> Self {
        Self {
            file: CachePadded::new(AtomicU64::new(0)),
            sym: CachePadded::new(AtomicU64::new(0)),
            dir: CachePadded::new(AtomicU64::new(0)),
        }
    }

    fn inc_file(&self) {
        self.file.fetch_add(1, atomic::Ordering::Relaxed);
    }

    fn inc_sym(&self) {
        self.sym.fetch_add(1, atomic::Ordering::Relaxed);
    }

    fn inc_dir(&self) {
        self.dir.fetch_add(1, atomic::Ordering::Relaxed);
    }

    /// splits the atom
    /// accumulates file, sym, dir counts
    fn accum(&self, values: DisplayTrackers) -> DisplayTrackers {
        DisplayTrackers {
            file: values.file + self.file.load(atomic::Ordering::Relaxed),
            sym: values.sym + self.sym.load(atomic::Ordering::Relaxed),
            dir: values.dir + self.dir.load(atomic::Ordering::Relaxed),
        }
    }
}

#[derive(Copy, Clone)]
struct DisplayTrackers {
    file: u64,
    sym: u64,
    dir: u64,
}

impl DisplayTrackers {
    fn new() -> Self {
        Self {
            file: 0,
            sym: 0,
            dir: 0,
        }
    }
}

impl fmt::Display for DisplayTrackers {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} file", self.file)?;
        if self.file != 1 {
            write!(f, "s")?;
        }

        write!(f, ", {} symlink", self.sym)?;
        if self.sym != 1 {
            write!(f, "s")?;
        }

        write!(f, ", and {} dir", self.dir)?;
        if self.dir != 1 {
            write!(f, "s")?;
        }

        Ok(())
    }
}

async fn cache_dir_async(
    dir: PathBuf,
    meta: Metadata,
    trackers: Arc<StatsTrackers>,
    spawner: mpsc::UnboundedSender<(PathBuf, Metadata)>,
    errors: mpsc::Sender<(PathBuf, io::Error)>,
) {
    match std::fs::read_dir(&dir) {
        Ok(dirs) => {
            for entry in dirs {
                let entry = match entry {
                    Ok(e) => e,
                    Err(err) => {
                        errors.send((dir.clone(), err)).await.unwrap();
                        continue;
                    }
                };

                let e_meta = match entry.metadata() {
                    Ok(m) => m,
                    Err(err) => {
                        errors.send((entry.path(), err)).await.unwrap();
                        continue;
                    }
                };

                if e_meta.is_symlink() {
                    trackers.inc_sym();
                } else if e_meta.is_file() {
                    trackers.inc_file();
                } else if e_meta.is_dir() {
                    trackers.inc_dir();
                    if e_meta.dev() == meta.dev() {
                        spawner.send((entry.path(), e_meta)).unwrap();
                    }
                }
            }
        }
        Err(e) => errors.send((dir, e)).await.unwrap(),
    }
}

#[derive(clap::Parser)]
#[clap(author = "ultrabear <bearodark@gmail.com>", version)]
/// A simple cli to load the metadata of given mountpoints into ram by reading them
struct Args {
    /// directories to traverse into
    #[arg(num_args = 1..)]
    dirs: Vec<PathBuf>,

    /// wait a number of seconds before starting
    #[arg(long)]
    wait: Option<u16>,
}

#[tokio::main]
async fn main() {
    let start = std::time::Instant::now();

    let parse = Args::parse();

    if let Some(wait) = parse.wait {
        sleep(Duration::from_secs(wait.into())).await;
    }

    let mtracker = TaskSpawner(500, TaskTracker::new());

    let (err_tx, mut err_rx) = mpsc::channel::<(PathBuf, io::Error)>(10);
    let initial_err = err_tx.clone();

    let (spawn_tx, mut spawn_rx) = mpsc::unbounded_channel::<(PathBuf, Metadata)>();
    let initial_spawn = spawn_tx.clone();

    let tracker = mtracker.clone();

    let spawner = tokio::spawn(async move {
        const TRACKERS: usize = 12;

        let statspool: [Arc<StatsTrackers>; TRACKERS] =
            core::array::from_fn(|_| Arc::new(StatsTrackers::new()));
        let mut tracker_i = 0;

        loop {
            if let Ok((dir, meta)) = spawn_rx.try_recv() {
                tracker
                    .spawn(cache_dir_async(
                        dir,
                        meta,
                        statspool[tracker_i].clone(),
                        spawn_tx.clone(),
                        err_tx.clone(),
                    ))
                    .await;
            // the spawner we hold is the only one left
            } else if spawn_rx.sender_strong_count() == 1 {
                // but its possible that something was added between try_recv and our strong count
                // check
                if let Ok((dir, meta)) = spawn_rx.try_recv() {
                    // there was something, keep going
                    tracker
                        .spawn(cache_dir_async(
                            dir,
                            meta,
                            statspool[tracker_i].clone(),
                            spawn_tx.clone(),
                            err_tx.clone(),
                        ))
                        .await;
                } else {
                    // there was nothing
                    break;
                }
            } else {
                // microsleep until the next recv is available
                sleep(Duration::from_micros(500)).await;
            }

            tracker_i += 1;
            tracker_i %= TRACKERS;
        }

        tracker.1.close();

        statspool
    });

    let errs = tokio::spawn(async move {
        while let Some((p, err)) = err_rx.recv().await {
            _ = writeln!(std::io::stderr().lock(), "{}: {err}", p.display());
        }
    });

    for dir in parse.dirs {
        let meta = match dir.metadata() {
            Ok(m) => m,
            Err(e) => {
                initial_err.send((dir, e)).await.unwrap();
                continue;
            }
        };

        initial_spawn.send((dir, meta)).unwrap();
    }

    drop((initial_err, initial_spawn));

    mtracker.1.wait().await;
    let trackers = spawner.await.unwrap();
    errs.await.unwrap();

    let counts = trackers
        .into_iter()
        .fold(DisplayTrackers::new(), |accum, it| it.accum(accum));

    _ = writeln!(
        std::io::stdout().lock(),
        "Processed {counts} in {:?}",
        start.elapsed()
    );
}
