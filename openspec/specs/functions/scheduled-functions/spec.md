# Scheduled Functions Specification

## Purpose

Run deployed functions on a schedule — cleanup jobs, reports, syncs — without an external cron, with a history developers can read.

## Requirements

### Requirement: Cron schedules for functions
An authorized member SHALL be able to attach a cron schedule, in UTC, to a deployed function with an optional request body and path, and to pause or delete it. Invalid expressions MUST be refused at save time, and a schedule MUST target only the function's active deployment.

#### Scenario: A developer schedules a nightly function
- **WHEN** an authorized member saves a valid schedule for a deployed function
- **THEN** the function is invoked at each due time with the configured request and the schedule shows its next run

### Requirement: Invocation history and overlap control
Each scheduled invocation SHALL be recorded with due time, start, duration, and outcome, retained like function metrics. A run that is still executing when the next is due SHALL cause the next to be skipped and recorded as skipped, never run concurrently.

#### Scenario: A run overlaps the next due time
- **WHEN** a scheduled invocation is still running at the next due time
- **THEN** the next invocation is recorded as skipped for overlap and the schedule continues afterwards
