use std::{path::PathBuf, process::ExitCode};

use futures::executor::block_on;
use mako_storage::{Durability, KvAdapter, SqliteAdapter, SqliteConfig, WriteBatch};

fn main() -> ExitCode {
    let mut arguments = std::env::args().skip(1);
    let Some(path) = arguments.next().map(PathBuf::from) else {
        return ExitCode::FAILURE;
    };
    let Some(identity) = arguments.next() else {
        return ExitCode::FAILURE;
    };
    let mut config = SqliteConfig::new(path, identity);
    config.disk_warning_free_bytes = 2;
    config.disk_critical_free_bytes = 1;
    let Ok(adapter) = SqliteAdapter::open(config) else {
        return ExitCode::FAILURE;
    };
    let mut batch = WriteBatch::new();
    batch.put(b"crash/acknowledged", b"durable");
    if block_on(adapter.write(batch, Durability::Sync)).is_err() {
        return ExitCode::FAILURE;
    }
    // Deliberately bypass Drop/shutdown to simulate abrupt process termination.
    std::process::exit(0)
}
