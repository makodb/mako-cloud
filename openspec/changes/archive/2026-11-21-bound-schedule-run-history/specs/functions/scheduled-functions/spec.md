## MODIFIED Requirements

### Requirement: Invocation history and overlap control
Each scheduled invocation SHALL be recorded with due time, start, duration, and outcome. Run history SHALL be retained for seven days and SHALL hold at most 1 000 runs per schedule, the oldest removed first. A run that is queued or still executing MUST NOT be pruned, and deleting a schedule SHALL delete its history. A run that is still executing when the next is due SHALL cause the next to be skipped and recorded as skipped, never run concurrently.

#### Scenario: A run overlaps the next due time
- **WHEN** a scheduled invocation is still running at the next due time
- **THEN** the next invocation is recorded as skipped for overlap and the schedule continues afterwards

#### Scenario: Run history is pruned
- **WHEN** a schedule holds more completed runs than its ceiling, or completed runs older than the retention window
- **THEN** the oldest runs beyond the ceiling and every run older than the window are removed, and a queued or executing run is kept
