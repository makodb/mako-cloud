## ADDED Requirements

### Requirement: Resumable multipart export download
The CLI SHALL download both legacy exports and multipart exports. Multipart downloads MUST validate ordered part metadata, each downloaded part and the completed file, retain protected verified progress across interruption, obtain fresh authorized grants as necessary, and publish the output path only after integrity verification.

#### Scenario: Resume interrupted download
- **WHEN** an authorized user reruns export for the same completed job and output path after an interrupted multipart download
- **THEN** verified cached parts are reused, missing or corrupt parts are fetched again, and the final JSON Lines file matches the manifest digest and byte count without whole-file memory buffering

#### Scenario: Cached parts belong to another export
- **WHEN** cached download progress does not match the requested job, scope and manifest
- **THEN** the CLI refuses to reuse it and leaves the final output untouched
