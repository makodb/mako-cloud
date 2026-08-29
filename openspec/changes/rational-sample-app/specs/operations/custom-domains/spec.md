## MODIFIED Requirements

### Requirement: Developer-managed custom domains
An authorized member SHALL be able to add a domain to a project, prove control of it through a DNS verification record, and have the platform obtain and renew a certificate for it. Until verified, the domain MUST NOT be served. A domain MUST map only to that project's API and function routes, MUST be removable, and MUST be revoked from serving if verification later fails. A domain SHALL carry an allowlist of application origins: the platform SHALL answer cross-origin preflights and emit cross-origin response headers on that domain only for a listed origin, MUST NOT emit them for any other origin or on the platform's own hostname, and MUST keep credentials and headers bounded to what the application API needs.

#### Scenario: A developer adds and verifies a domain
- **WHEN** an authorized member adds a domain and publishes the verification record
- **THEN** the platform verifies it, obtains a certificate, and serves the project's API and functions on the domain within the stated objective

#### Scenario: A domain fails re-verification
- **WHEN** the verification record is removed after the domain was serving
- **THEN** the platform stops serving the domain, marks it unverified, and tells the developer why

#### Scenario: A browser app on a listed origin calls the domain
- **WHEN** an application served from an origin the domain lists sends a preflighted request to the domain
- **THEN** the preflight and the request succeed with cross-origin headers naming that origin, while the same request from an unlisted origin receives no cross-origin headers and the browser blocks it
