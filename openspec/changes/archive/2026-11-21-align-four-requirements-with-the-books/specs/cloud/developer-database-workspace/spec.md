## MODIFIED Requirements

### Requirement: API and RxDB connection guidance
The workspace SHALL show the selected environment's public API endpoint, the identifier of its active public project key, collection identifier, active schema version, supported RxDB client package and version range, and generated setup examples matching the published client API. A public key's value is shown once, when the key is issued, and MUST NOT be shown, stored or reconstructed by this page; examples SHALL carry a placeholder where the value goes. The page MUST NOT reveal service keys, signing material, function secrets, or other privileged credentials.

#### Scenario: Developer copies an RxDB setup example
- **WHEN** a developer selects a collection and supported client version
- **THEN** the workspace generates a copyable example containing the correct endpoint, a placeholder for the public key value, collection, schema version, authentication placeholder, and required replication configuration

#### Scenario: No public key is active
- **WHEN** the selected environment has no usable public client key
- **THEN** the page reports setup as incomplete and links an authorized member to create or activate a public key without substituting a service credential
