## ADDED Requirements

### Requirement: Canonical Mako Cloud executable
The CLI package SHALL install `mako-cloud` as its sole executable name and MUST NOT install a `mako` compatibility alias. Generated help, usage errors, recovery instructions, repository documentation, and executable examples SHALL identify the command as `mako-cloud`.

#### Scenario: The CLI package is installed
- **WHEN** a developer installs or links the CLI package
- **THEN** `mako-cloud` is available and the package does not create a `mako` executable

#### Scenario: The CLI tells a developer how to continue
- **WHEN** the CLI renders help, rejects invalid arguments, or prints a recovery command
- **THEN** every displayed invocation begins with `mako-cloud`

## MODIFIED Requirements

### Requirement: Complete command coverage of the developer management surface
The `mako-cloud` command SHALL expose every developer-facing management, developer-auth, explorer, data-job, workspace, sync, and backup operation of the public API as a command, and the repository SHALL carry an automated check that fails when an operation of the public API in that set has no command or a command maps to no operation. Operator operations and application-runtime operations MUST be excluded from the set explicitly, by named path prefixes.

#### Scenario: The API grows an operation the CLI does not expose
- **WHEN** a developer-facing operation is added to the public API document without a command mapping
- **THEN** the parity check fails and names the operation

#### Scenario: A developer completes a console workflow from the terminal
- **WHEN** a signed-in developer creates a project, waits for it to become active, publishes a collection schema, activates a policy, issues a public key, deploys a function, and reads its logs using only `mako-cloud` commands
- **THEN** each step succeeds with the same authorization and audit events as the console would produce

### Requirement: Terminal authentication and credential storage
The CLI SHALL sign a developer in with the hosted registration flow's credentials and store the resulting session in a per-profile credential file readable only by the user, refusing to use a file that other users can read; SHALL refresh sessions before expiry; SHALL accept a token from the environment for non-interactive use without ever writing it to disk; SHALL sign out by revoking the session server-side; and SHALL obtain step-up grants by prompting for the password on a terminal, never accepting a password as a command argument.

#### Scenario: A developer signs in and the session is kept safely
- **WHEN** a developer runs `mako-cloud auth login` and completes it
- **THEN** subsequent commands use the stored session, the credential file is readable only by its owner, and `mako-cloud auth logout` revokes the session before deleting it

#### Scenario: CI runs with a token from the environment
- **WHEN** `MAKO_TOKEN` is set and a command runs without a terminal
- **THEN** the command authenticates with that token, nothing is written to the credential file, and a command the token's permissions do not allow fails with the authorization exit code naming the permission

#### Scenario: A step-up action without a terminal
- **WHEN** a command needing a step-up grant runs without a terminal and without a configured step-up source
- **THEN** it fails with the authentication exit code and an explanation, creating nothing

### Requirement: Composed flows for deploy, data transfer, and lifecycle waits
The CLI SHALL provide `mako-cloud functions deploy` (bundle, upload, create deployment, health check, promote unless declined), `mako-cloud data export` and `mako-cloud data import` (data jobs with grants, dry run, and explicit confirmation), and `--wait` on lifecycle commands that polls to a terminal state or a deadline, each printing the identifiers it produced so a failed run can be resumed with lower-level commands.

#### Scenario: A function is deployed in one command
- **WHEN** a developer runs `mako-cloud functions deploy` on a function directory
- **THEN** the bundle is uploaded, a deployment is created, its health is checked, it is promoted, and each produced identifier is printed

#### Scenario: A lifecycle wait times out
- **WHEN** a developer runs a create command with `--wait` and the resource does not reach a terminal state by the deadline
- **THEN** the command exits with the wait-timeout code after printing the resource identifier and its last observed state
