# Custom Domains Specification

## Purpose

Serve a project's API and functions on the developer's own domain with a managed certificate, so applications never expose the platform's hostname.

## Requirements

### Requirement: Developer-managed custom domains
An authorized member SHALL be able to add a domain to a project, prove control of it through a DNS verification record, and have the platform obtain and renew a certificate for it. Until verified, the domain MUST NOT be served. A domain MUST map only to that project's API and function routes, MUST be removable, and MUST be revoked from serving if verification later fails.

#### Scenario: A developer adds and verifies a domain
- **WHEN** an authorized member adds a domain and publishes the verification record
- **THEN** the platform verifies it, obtains a certificate, and serves the project's API and functions on the domain within the stated objective

#### Scenario: A domain fails re-verification
- **WHEN** the verification record is removed after the domain was serving
- **THEN** the platform stops serving the domain, marks it unverified, and tells the developer why
