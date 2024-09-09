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

use core::{
    fmt,
    future::Future,
    sync::atomic::{self, AtomicU64},
    time::Duration,
};

use alloc::sync::Arc;

use std::{
    fs::Metadata,
    io::{self, Write},
    os::unix::fs::MetadataExt,
    path::PathBuf,
};

use clap::Parser;
use crossbeam_utils::CachePadded;
use tokio::{sync::mpsc, task, time::sleep};
use tokio_util::task::{task_tracker::TaskTrackerWaitFuture, TaskTracker};

#[derive(Clone)]
/// A `TaskTracker` based spawn limiter, only lim tasks may live at a time when spawned by this
/// object
struct TaskSpawner {
    /// The most amount of tasks that can be alive at once
    lim: usize,
    /// task tracking primitive
    track: TaskTracker,
}

impl TaskSpawner {
    /// creates a new `TaskSpawner` with a set limit `lim`
    fn new(lim: usize) -> Self {
        Self {
            lim,
            track: TaskTracker::new(),
        }
    }

    /// Spawns a future on this tracker after waiting for the amount of tasks to be less than lim
    async fn spawn<F: Future + Send + 'static>(&self, task: F) -> task::JoinHandle<F::Output>
    where
        F::Output: Send + 'static,
    {
        while self.track.len() > self.lim {
            sleep(Duration::from_micros(500)).await;
        }

        self.track.spawn(task)
    }

    /// Closes the tracker, no new tasks may be spawned after this point
    fn close(&self) -> bool {
        self.track.close()
    }

    /// Waits for all tasks to be completed and close to be called
    fn wait(&self) -> TaskTrackerWaitFuture {
        self.track.wait()
    }
}

/// An atomic structure that tracks file/sym/dir counts during inode traversal
#[derive(Debug)]
struct Stats {
    /// file count
    file: CachePadded<AtomicU64>,
    /// symlink count
    sym: CachePadded<AtomicU64>,
    /// directory count
    dir: CachePadded<AtomicU64>,
}

impl Stats {
    /// Creates a new stat tracker
    const fn new() -> Self {
        Self {
            file: CachePadded::new(AtomicU64::new(0)),
            sym: CachePadded::new(AtomicU64::new(0)),
            dir: CachePadded::new(AtomicU64::new(0)),
        }
    }

    /// increments file counter
    fn inc_file(&self) {
        self.file.fetch_add(1, atomic::Ordering::Relaxed);
    }

    /// increments symlink counter
    fn inc_sym(&self) {
        self.sym.fetch_add(1, atomic::Ordering::Relaxed);
    }

    /// increments directory counter
    fn inc_dir(&self) {
        self.dir.fetch_add(1, atomic::Ordering::Relaxed);
    }

    /// splits the atom
    /// accumulates file, sym, dir counts into a `DisplayStats`
    fn accum(&self, values: DisplayStats) -> DisplayStats {
        DisplayStats {
            file: values.file + self.file.load(atomic::Ordering::Relaxed),
            sym: values.sym + self.sym.load(atomic::Ordering::Relaxed),
            dir: values.dir + self.dir.load(atomic::Ordering::Relaxed),
        }
    }
}

#[derive(Copy, Clone)]
/// Non atomic structure to display accumulated statistics
struct DisplayStats {
    /// file count
    file: u64,
    /// symlink count
    sym: u64,
    /// directory count
    dir: u64,
}

impl DisplayStats {
    /// Create a new `DisplayStats` instance
    const fn new() -> Self {
        Self {
            file: 0,
            sym: 0,
            dir: 0,
        }
    }
}

impl fmt::Display for DisplayStats {
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

/// Caches the provided directory with accompanying metadata.
///
/// Increments statistics and sends any found directories to the spawner channel.
/// Any errors encountered are sent to the errors channel.
async fn cache_dir(
    dir: PathBuf,
    meta: Metadata,
    trackers: Arc<Stats>,
    spawner: mpsc::UnboundedSender<(PathBuf, Metadata)>,
    errors: mpsc::Sender<(PathBuf, io::Error)>,
) {
    let send_err = |dir, err| async {
        errors
            .send((dir, err))
            .await
            .expect("error channel must be open until spawner ends");
    };

    match std::fs::read_dir(&dir) {
        Ok(dirs) => {
            for entry in dirs {
                let entry = match entry {
                    Ok(e) => e,
                    Err(err) => {
                        send_err(dir.clone(), err).await;
                        continue;
                    }
                };

                let e_meta = match entry.metadata() {
                    Ok(m) => m,
                    Err(err) => {
                        send_err(entry.path(), err).await;
                        continue;
                    }
                };

                if e_meta.is_symlink() {
                    trackers.inc_sym();
                } else if e_meta.is_file() {
                    trackers.inc_file();
                } else if e_meta.is_dir() {
                    trackers.inc_dir();

                    #[cfg(not(unix))]
                    compile_error!(
                        "cannot compile, filesystem device ids not supported on this platform"
                    );

                    #[cfg(unix)]
                    if e_meta.dev() == meta.dev() {
                        spawner
                            .send((entry.path(), e_meta))
                            .expect("spawner channel must be open until spawner ends");
                    }
                }
            }
        }
        Err(e) => send_err(dir, e).await,
    }
}

#[derive(clap::Parser)]
#[clap(author = "ultrabear <bearodark@gmail.com>", version)]
/// A simple cli to load the metadata of given mountpoints into ram by reading them
struct Args {
    /// directories to traverse into
    #[arg(num_args = 1..)]
    dirs: Vec<PathBuf>,
}

#[tokio::main]
async fn main() {
    let start = std::time::Instant::now();

    let parse = Args::parse();

    let (err_tx, mut err_rx) = mpsc::channel::<(PathBuf, io::Error)>(50);
    let initial_err = err_tx.clone();

    let (spawn_tx, mut spawn_rx) = mpsc::unbounded_channel::<(PathBuf, Metadata)>();
    let initial_spawn = spawn_tx.clone();

    let tracker = TaskSpawner::new(500);
    let main_tracker = tracker.clone();

    let spawner = tokio::spawn(async move {
        /// The number of statistics objects to be created for atomic load balancing
        const NUM_STATS: usize = 12;

        let statspool: [Arc<Stats>; NUM_STATS] = core::array::from_fn(|_| Arc::new(Stats::new()));
        let mut stats_idx = 0;

        loop {
            if let Ok((dir, meta)) = spawn_rx.try_recv() {
                tracker
                    .spawn(cache_dir(
                        dir,
                        meta,
                        statspool[stats_idx].clone(),
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
                        .spawn(cache_dir(
                            dir,
                            meta,
                            statspool[stats_idx].clone(),
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

            stats_idx += 1;
            stats_idx %= NUM_STATS;
        }

        tracker.close();

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
                initial_err
                    .send((dir, e))
                    .await
                    .expect("channel cannot be closed until initial_err is");
                continue;
            }
        };

        initial_spawn
            .send((dir, meta))
            .expect("channel cannot be closed until initial_spawn is");
    }

    drop((initial_err, initial_spawn));

    main_tracker.wait().await;
    // note that spawner must be closed first: it owns err_tx which errs waits on
    let trackers = spawner
        .await
        .expect("no panic should have occurred in this thread");
    errs.await
        .expect("no panic should have occurred in this thread");

    let counts = trackers
        .into_iter()
        .fold(DisplayStats::new(), |accum, it| it.accum(accum));

    _ = writeln!(
        std::io::stdout().lock(),
        "Processed {counts} in {:?}",
        start.elapsed()
    );
}
