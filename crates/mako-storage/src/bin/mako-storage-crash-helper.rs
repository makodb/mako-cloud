use std::{num::NonZeroUsize, path::PathBuf, process::ExitCode, sync::Arc, time::Duration};

use futures::executor::block_on;
use mako_storage::{
    Durability, KvAdapter, ProductionRocksDb, ProductionRocksDbConfig, ProductionVolumeIdentity,
    WriteBatch,
};
use rocksdb::{DB, Options};

fn main() -> ExitCode {
    let mut arguments = std::env::args().skip(1);
    let Some(mode) = arguments.next() else {
        return ExitCode::FAILURE;
    };
    let Some(database_path) = arguments.next().map(PathBuf::from) else {
        return ExitCode::FAILURE;
    };
    if arguments.next().is_some() {
        return ExitCode::FAILURE;
    }
    let identity =
        match ProductionVolumeIdentity::new("mako-data-plane", "mako-data-plane-crash-test") {
            Ok(identity) => identity,
            Err(_) => return ExitCode::FAILURE,
        };
    let storage = match block_on(ProductionRocksDb::open(ProductionRocksDbConfig {
        database_path: database_path.clone(),
        identity,
        maximum_batch_operations: NonZeroUsize::new(10_000).expect("constant"),
        maximum_scan_items: NonZeroUsize::new(10_000).expect("constant"),
        transaction_lock_timeout: Duration::from_millis(100),
        transaction_expiration: Duration::from_secs(5),
        disk_warning_free_bytes: 2,
        disk_critical_free_bytes: 1,
    })) {
        Ok(storage) => storage,
        Err(_) => return ExitCode::FAILURE,
    };

    let mut batch = WriteBatch::new();
    batch.put(b"crash/sentinel", b"acknowledged");
    if block_on(storage.adapter().write(batch, Durability::Sync)).is_err() {
        return ExitCode::FAILURE;
    }
    if mode == "ack-crash" {
        std::process::exit(0);
    }
    if mode != "compaction-crash" {
        return ExitCode::FAILURE;
    }

    for group in 0_u32..16 {
        let mut batch = WriteBatch::new();
        for item in 0_u32..128 {
            let mut value = vec![0_u8; 8 * 1024];
            value[..4].copy_from_slice(&group.to_be_bytes());
            batch.put(
                format!("compaction/{group:04}/{item:04}").into_bytes(),
                value,
            );
        }
        if block_on(storage.adapter().write(batch, Durability::Sync)).is_err() {
            return ExitCode::FAILURE;
        }
    }
    if block_on(storage.graceful_shutdown()).is_err() {
        return ExitCode::FAILURE;
    }

    let mut options = Options::default();
    options.create_if_missing(false);
    let database = match DB::open(&options, database_path) {
        Ok(database) => Arc::new(database),
        Err(_) => return ExitCode::FAILURE,
    };
    let compactor = Arc::clone(&database);
    std::thread::spawn(move || {
        compactor.compact_range::<&[u8], &[u8]>(None, None);
    });
    std::thread::sleep(Duration::from_millis(1));
    std::process::exit(0);
}
