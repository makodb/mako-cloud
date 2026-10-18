## 1. Storage accounting
- [x] 1.1 Implement canonical range registration, durable sharded counters, atomic mutation deltas and restart fencing.
- [x] 1.2 Implement bounded resumable reconciliation and non-polluting scans with explicit block cache budget.
- [x] 1.3 Add regression coverage for concurrent writes, duplicate operations, failed CAS, restart/rollback, and bounded progress.
## 2. Telemetry integration
- [x] 2.1 Integrate pending samples and fix dirty-mark/cooldown scheduling with regression coverage.
- [x] 2.2 Document behavior and update traceability; run relevant Rust and specification checks.
## 3. Release
- [ ] 3.1 Build matching Linux artifacts and install a verified immutable release.
- [ ] 3.2 Deploy, verify readiness, restore schedules and compare live I/O and request timing.
