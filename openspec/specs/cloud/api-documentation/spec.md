# API Documentation Specification

## Purpose

Show every environment its own API: the collections it actually has, the shapes they actually enforce, and working snippets with its own keys — so a developer never leaves the console to learn how to talk to their database.

## Requirements

### Requirement: Generated per-environment API documentation
The console SHALL generate documentation for an environment from its live collections, schemas, indexes, policies, and functions: for each collection the document shape, the allowed operations under the current policies, and query examples; for auth the sign-up, sign-in, and session endpoints; for functions their routes. Documentation MUST reflect the environment's current state at the observation time it names, and MUST NOT include secret values.

#### Scenario: A developer reads the docs for a collection
- **WHEN** an authorized developer opens API documentation for an environment with a schema-bearing collection
- **THEN** the collection's document shape, allowed operations, and example requests are shown, generated from the current schema and policies

### Requirement: Quickstarts with the environment's own keys
The documentation SHALL offer quickstarts for the supported clients using the environment's API URL and public key, with the key shown only to members allowed to see it and never a service credential.

#### Scenario: A developer copies a quickstart
- **WHEN** an authorized developer opens a quickstart for a supported client
- **THEN** the snippet carries the environment's API URL and public key and runs against the environment as shown
