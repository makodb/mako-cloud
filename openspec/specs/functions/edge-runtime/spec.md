# Edge Runtime Specification

## Purpose

Let projects run short-lived TypeScript or JavaScript HTTP functions in an isolated Deno-compatible runtime close to users, with secure access to Mako Cloud services.

## Requirements

### Requirement: Deno-compatible function contract
The platform SHALL run TypeScript and JavaScript entrypoints using a Deno-compatible Fetch API contract. Functions SHALL be able to process HTTP requests, return standard responses and streams, use supported npm modules and WebAssembly, and make outbound network requests subject to project policy.

#### Scenario: TypeScript function handles a request
- **WHEN** a deployed function exports a valid request handler
- **THEN** an invocation supplies a standards-based Request and returns the handler's Response to the caller

#### Scenario: Unsupported runtime feature is used
- **WHEN** a function depends on an unavailable native or multi-threaded runtime feature
- **THEN** deployment or invocation fails with a diagnostic identifying the unsupported capability

### Requirement: Versioned deployment lifecycle
Developers SHALL be able to create, bundle, validate, deploy, list, promote, roll back, and delete immutable function versions. Traffic switching between healthy versions MUST be atomic, and a failed deployment MUST leave the prior active version serving.

#### Scenario: New version passes validation
- **WHEN** an authorized developer deploys a valid function bundle
- **THEN** the platform creates an immutable version and can atomically promote it to active

#### Scenario: Rollback is requested
- **WHEN** an authorized developer selects a previously healthy version
- **THEN** new invocations route to that version without mutating its source bundle

### Requirement: Function invocation gateway
Each deployed function SHALL have a stable project-scoped HTTPS endpoint supporting standard HTTP methods and streaming responses. The gateway SHALL validate a project JWT by default, with an explicit per-function setting for public webhook-style invocation.

#### Scenario: Protected function is called without a token
- **WHEN** a function requiring authentication receives no valid project JWT
- **THEN** the gateway rejects the request before user code executes

#### Scenario: Public webhook function is called
- **WHEN** a function version is explicitly configured for public invocation
- **THEN** the gateway invokes it without a user JWT while still applying project routing, quotas, and request limits

### Requirement: Caller-aware Mako SDK
The runtime SHALL provide a supported SDK for document and auth operations. By default it SHALL propagate the verified invoking identity so document policies apply; privileged service access SHALL require an explicit service credential supplied through a secret.

#### Scenario: Function reads application data
- **WHEN** a protected function uses the default document client
- **THEN** results are filtered by the invoking user's active document policies

#### Scenario: Function performs privileged maintenance
- **WHEN** a function explicitly initializes a client with a valid service secret
- **THEN** privileged access is allowed and audited as a service operation

### Requirement: Encrypted project secrets
Authorized developers SHALL be able to create, rotate, list metadata for, and delete project function secrets. Secret values MUST be encrypted at rest, exposed only to selected runtime deployments as environment variables, redacted from APIs and logs, and never returned after creation.

#### Scenario: Secret is created
- **WHEN** an authorized developer submits a new secret value
- **THEN** the response confirms its name and version without echoing the value

#### Scenario: Function logs a known secret
- **WHEN** runtime output contains an exact active secret value detectable by the platform
- **THEN** the log pipeline redacts it before storage or display

### Requirement: Workload isolation and limits
Every invocation SHALL run within an isolated project context with configurable CPU time, wall time, memory, request-body, response-body, and concurrency limits. Limit violations MUST terminate or reject only the offending workload and MUST NOT expose another project's memory, files, environment, or traffic.

#### Scenario: Function exceeds CPU limit
- **WHEN** an invocation consumes more CPU than allowed
- **THEN** the runtime terminates it, returns a limit error, and records the event without destabilizing other projects

#### Scenario: Function attempts undeclared secret access
- **WHEN** user code reads a secret not attached to its deployment
- **THEN** the value is absent and no cross-project lookup occurs

### Requirement: Regional execution and routing
Projects SHALL be able to select supported deployment regions, and the gateway SHALL route invocations to the nearest healthy selected region while preserving project configuration and active function version. If no selected region is healthy, the request SHALL fail rather than run in an unauthorized region.

#### Scenario: Nearest region is unavailable
- **WHEN** another selected region is healthy
- **THEN** the gateway routes new invocations to that healthy region and records the failover

### Requirement: Function observability
The platform SHALL capture structured logs, invocation counts, latency, status, resource use, deployment version, region, and correlation identifiers. Authorized project members SHALL be able to query these signals within retention and quota limits.

#### Scenario: Invocation fails
- **WHEN** a function throws an uncaught error
- **THEN** the caller receives a correlation identifier and authorized developers can find sanitized logs and metrics for that invocation

### Requirement: Local development parity
The platform SHALL provide a local command or development service that runs the same request contract, environment mapping, JWT verification option, and supported runtime APIs used in hosted execution.

#### Scenario: Developer serves a function locally
- **WHEN** the developer supplies local project configuration and secrets
- **THEN** the function can be invoked locally with behavior compatible with hosted deployment apart from documented regional infrastructure differences

### Requirement: Bounded execution model
The MVP SHALL support request-driven and streaming-response workloads but SHALL reject designs that require unbounded background processing after the request lifecycle. Long-running jobs MUST be delegated to an external worker system.

#### Scenario: Function attempts to run past its wall limit
- **WHEN** background work continues beyond the configured invocation lifetime
- **THEN** the runtime terminates that work and records a limit event

