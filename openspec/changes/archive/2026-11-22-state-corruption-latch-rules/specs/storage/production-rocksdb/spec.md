## ADDED Requirements

### Requirement: A corruption signal stops mutations until restart
After the first corruption error that RocksDB reports to any operation on an open database (a read, a scan, a snapshot read, a size or count walk, a write, or the health probe), the service that owns the database SHALL report storage health as unavailable and readiness as false, and SHALL refuse every mutation of that database with a non-retryable error. It SHALL keep serving reads, so that the volume can be inspected and preserved. The state SHALL last until the process is restarted and MUST NOT clear by itself. The readiness detail the service publishes SHALL name the documented recovery procedure and MUST NOT contain vendor error text, keys, values, or storage paths. An I/O error alone SHALL be counted and reported to its caller as retryable, and MUST NOT by itself fail readiness or refuse mutations.

#### Scenario: A read meets a damaged block
- **WHEN** a read on an open database returns a corruption error
- **THEN** readiness turns false with a detail that names the RocksDB incident runbook, and a write that follows is refused as non-retryable corruption without being attempted

#### Scenario: Reads continue after the signal
- **WHEN** a corruption signal has been recorded and a caller reads a key or opens a snapshot
- **THEN** the read is served, and the refused mutations are not counted as further corruption signals

#### Scenario: The state lasts until restart
- **WHEN** readiness is asked again after a corruption signal, and later the process is restarted on the same volume
- **THEN** readiness stays false for as long as the process runs, and the restarted process is ready until an operation meets corruption again

#### Scenario: An I/O error alone
- **WHEN** an operation returns an I/O error and no corruption has been signalled
- **THEN** the error is counted and returned as retryable, readiness stays true, and writes are still accepted
