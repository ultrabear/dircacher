use std::{
    fs::Metadata,
    future::Future,
    io::{self, Write},
    os::unix::fs::MetadataExt,
    path::PathBuf,
    time::Duration,
};

use clap::Parser;
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
            sleep(Duration::from_millis(1)).await;
        }

        self.1.spawn(task)
    }
}

async fn cache_dir_async(
    dir: PathBuf,
    meta: Metadata,
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

                if e_meta.is_dir() && e_meta.dev() == meta.dev() {
                    spawner.send((entry.path(), e_meta)).unwrap();
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

    tokio::spawn(async move {
        loop {
            if let Ok((dir, meta)) = spawn_rx.try_recv() {
                tracker
                    .spawn(cache_dir_async(dir, meta, spawn_tx.clone(), err_tx.clone()))
                    .await;
            // the spawner we hold is the only one left
            } else if spawn_rx.sender_strong_count() == 1 {
                // but its possible that something was added between try_recv and our strong count
                // check
                if let Ok((dir, meta)) = spawn_rx.try_recv() {
                    // there was something, keep going
                    tracker
                        .spawn(cache_dir_async(dir, meta, spawn_tx.clone(), err_tx.clone()))
                        .await;
                } else {
                    // there was nothing
                    break;
                }
            } else {
                // microsleep until the next recv is available
                sleep(Duration::from_micros(500)).await;
            }
        }

        tracker.1.close();
    });

    tokio::spawn(async move {
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
}
