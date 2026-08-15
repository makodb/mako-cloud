## Purpose

Provide a deterministic local tenant bootstrap and an automated end-to-end verification that Mako Cloud's application happy path — application-user sign-up, sign-in, document push, and document pull — actually succeeds against the real service binaries rather than against mocks.

## ADDED Requirements

### Requirement: Documented local quickstart starts the services
The documented local development quickstart SHALL bring the control plane and data plane to a ready state using only the files and commands the documentation names. Every configuration value a service requires in order to pass readiness MUST be present in the shipped example configuration, or the documentation MUST state how to produce it. A service that refuses to start because a documented example omitted required configuration is a defect in that example.

#### Scenario: Operator follows the quickstart verbatim
- **WHEN** a developer performs the documented preparation and start steps on a clean checkout without editing any file beyond what the documentation instructs
- **THEN** both services reach a ready state and report readiness successfully

#### Scenario: Required configuration is absent
- **WHEN** configuration required for readiness is missing
- **THEN** the service refuses to start with an addressed configuration error naming the field path, and the documentation names the value that must be supplied

### Requirement: Local tenant bootstrap
A bootstrap capability SHALL create a complete, usable local tenant — a developer identity, organization, project, environment, public project key, and collection — without requiring outbound mail, authenticated TLS SMTP, or an operator wait-list decision. The bootstrap SHALL be deterministic, so that repeated runs against clean state produce the same identifiers, and idempotent, so that a repeated run against existing state neither duplicates nor corrupts records. It MUST write only to stores it exclusively owns at the time it runs.

#### Scenario: Bootstrap produces a usable tenant
- **WHEN** the bootstrap runs against prepared local state
- **THEN** it reports the created project, environment, collection, and public project key, and those values authenticate successfully against the running data plane

#### Scenario: Bootstrap runs a second time
- **WHEN** the bootstrap runs again against state it already created
- **THEN** it completes without duplicating records and reports the same identifiers

### Requirement: Bootstrap is refused outside local environments
The bootstrap capability SHALL fail closed unless the resolved deployment environment is local. It MUST NOT be usable as a provisioning path for a hosted or production environment, and it MUST NOT weaken any authentication, wait-list, or entitlement control that a non-local environment enforces.

#### Scenario: Bootstrap is invoked against a non-local environment
- **WHEN** the bootstrap is invoked while the resolved environment is anything other than local
- **THEN** it refuses to modify any state and exits with a non-zero status and an explicit reason

### Requirement: End-to-end application happy path is verified automatically
An automated test SHALL start the real control-plane and data-plane binaries and exercise the application happy path over HTTP against them, with no mocked transport, no in-process fake backend, and no stubbed responses. It SHALL cover application-user sign-up, sign-in with the issued credentials, a document push, and a document pull that returns the pushed document. The test MUST assert the document round-trips with its written content, and MUST fail if any step returns an error status.

#### Scenario: Happy path succeeds
- **WHEN** the smoke test runs against a bootstrapped local tenant
- **THEN** sign-up, sign-in, push, and pull all succeed and the pulled document matches what was pushed

#### Scenario: A step of the happy path regresses
- **WHEN** any step of the flow returns an error status or the pulled document does not match what was pushed
- **THEN** the test fails and identifies the step that broke

#### Scenario: Credentials are required
- **WHEN** the same document operations are attempted without the issued session credentials
- **THEN** they are refused, proving the successful run depended on real authentication

### Requirement: Smoke verification gates continuous integration and is traceable
The end-to-end smoke verification SHALL run as a gating check in continuous integration, so that a regression in the happy path fails the build. Its scenarios SHALL appear in the requirements traceability matrix. A scenario whose only automated coverage runs against a mocked backend MUST NOT be recorded as verifying real end-to-end behavior.

#### Scenario: Happy path breaks on a branch
- **WHEN** a change breaks sign-up, sign-in, push, or pull
- **THEN** the continuous integration run fails on the smoke check

#### Scenario: Coverage is claimed for a mocked test
- **WHEN** a traceability entry cites a test whose backend is mocked
- **THEN** the entry states that the coverage is mocked rather than end-to-end
