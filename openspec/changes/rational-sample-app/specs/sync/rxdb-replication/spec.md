## MODIFIED Requirements

### Requirement: Supported RxDB client integration
The platform SHALL publish a versioned client integration that connects an RxDB collection to Mako Cloud using RxDB's pull handler, push handler, and live pull stream contracts. The integration SHALL be the supported application data interface for the MVP. The integration SHALL support durable local storage: replication checkpoints and security state SHALL be persisted alongside the application's data so that a restarted application resumes from where it stopped instead of pulling everything again, and a security reset SHALL clear that persisted state. The integration SHALL also cover every application sign-in method the platform offers — password, external provider, and magic link — and object storage access under the same session, so an application does not hand-build those requests.

#### Scenario: Application starts replication
- **WHEN** an application supplies a project endpoint, collection, schema version, public project key, and application-user session
- **THEN** the integration starts bidirectional replication and exposes RxDB replication state, conflicts, and errors to the application

#### Scenario: Application restarts on durable storage
- **WHEN** an application using durable local storage is closed and reopened
- **THEN** its documents are available before any network request, replication resumes from the persisted checkpoint, and only changes since that checkpoint are pulled

#### Scenario: Application signs in through a provider with the integration
- **WHEN** an application uses the integration's provider sign-in helper and the browser returns from the provider
- **THEN** the helper completes the exchange, stores the session with the configured persistence, and replication proceeds under it
