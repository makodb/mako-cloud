## MODIFIED Requirements

### Requirement: Navigation shell
Every environment-level screen SHALL be mounted in a persistent navigation shell whose destinations are Database (collections, schemas, indexes), Explorer, Auth (application users, providers), Storage, Functions (deployments, secrets, schedules), Sync (replication diagnostics, webhooks), Logs, Observability, API & Keys, Backups, and Settings. The active team, project, environment, and developer identity MUST remain visible; destinations the deployment does not provide MUST be shown as unavailable rather than hidden; and an existing deep link MUST open inside the shell with its context intact. The shell and every destination SHALL render through the shared design system, and the shell SHALL honour the developer's light or dark theme preference, kept per device, defaulting to the device's own preference.

#### Scenario: Every destination is one click away
- **WHEN** an authorized developer is anywhere inside an environment
- **THEN** each destination is reachable from the shell without returning to a parent page, and the current destination is indicated

#### Scenario: A deep link opens inside the shell
- **WHEN** a developer opens a bookmarked environment URL
- **THEN** the screen renders inside the shell with the team, project, and environment context shown

#### Scenario: The console follows the developer's theme
- **WHEN** a developer chooses the dark theme and opens any destination or reloads
- **THEN** the shell and the destination render with the dark palette, and the choice is remembered on that device
