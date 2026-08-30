## MODIFIED Requirements

### Requirement: Cron schedules for functions
An authorized member SHALL be able to attach a cron schedule, in UTC, to a deployed function with an optional request body, path, and headers, and to pause or delete it. Invalid expressions MUST be refused at save time, and a schedule MUST target only the function's active deployment. The headers naming the schedule, the run, and the due time SHALL be the scheduler's alone: the platform sets them on the invocation it makes and strips them from every public request, so a function that behaves differently when scheduled is trusting the platform and not its caller. A scheduled function's route remains public, so a function whose work only the schedule should start MUST refuse an invocation that does not carry them, or hold a secret of its own that the schedule carries.

#### Scenario: A developer schedules a nightly function
- **WHEN** an authorized member saves a valid schedule for a deployed function
- **THEN** the function is invoked at each due time with the configured request and the schedule shows its next run

#### Scenario: A public caller claims to be a schedule
- **WHEN** a request arrives on the function's public route carrying the scheduler's own headers
- **THEN** the function is invoked without them and is audited as the caller it actually was
