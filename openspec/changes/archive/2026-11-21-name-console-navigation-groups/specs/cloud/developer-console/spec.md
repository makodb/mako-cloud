## MODIFIED Requirements

### Requirement: Navigation shell
Every environment-level screen SHALL be mounted in a persistent navigation shell whose destinations are grouped under four headings:

- Database: Overview, Data (the data explorer), Collections, Policies, Sync, and Backups;
- Application: Users, Auth providers, Email templates, Storage, Functions (deployments, secrets, logs, and each function's schedules), and Webhooks;
- Observe: Observability, Logs, Usage, and Activity;
- Configure: API & Connect, API keys, API docs, and Settings.

The active personal or team ownership, project, environment, and developer identity MUST remain visible, and navigation SHALL follow the permissions the platform returns for the developer. Destinations the deployment does not provide MUST be shown as unavailable rather than hidden; and an existing deep link MUST open inside the shell with its context intact. On a narrow screen a "Go to" control SHALL select a destination without the wide sidebar. The shell and every destination SHALL render through the shared design system, and the shell SHALL honour the developer's light or dark theme preference, kept per device, defaulting to the device's own preference. Signed-in console sidebars MUST NOT include a Documentation destination.

#### Scenario: Every destination is one click away
- **WHEN** an authorized developer is anywhere inside an environment
- **THEN** each destination is reachable from the shell without returning to a parent page, and the current destination is indicated

#### Scenario: A deep link opens inside the shell
- **WHEN** a developer opens a bookmarked environment URL
- **THEN** the screen renders inside the shell with personal or team ownership, project, and environment context shown

#### Scenario: The console follows the developer's theme
- **WHEN** a developer chooses the dark theme and opens any destination or reloads
- **THEN** the shell and the destination render with the dark palette, and the choice is remembered on that device
