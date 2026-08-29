# Auth Providers Specification

## Purpose

Let applications sign users in the ways users expect — social and enterprise providers, magic links — and control the emails those flows send, per environment.

## Requirements

### Requirement: External sign-in providers
An authorized member SHALL be able to enable OAuth and OpenID Connect providers for an environment by supplying client identifiers and secret references, and to disable them. Application users SHALL sign in through an enabled provider and receive the same session as password sign-in, with the provider identity linked to the application user by verified email and provider subject. Secrets MUST be stored as references and MUST NOT be returned once saved.

#### Scenario: An application user signs in with a provider
- **WHEN** an application user completes an enabled provider's flow with a verified email
- **THEN** an application-user session is issued for the matching or newly created user and the sign-in is recorded as an authentication event

#### Scenario: A disabled provider is used
- **WHEN** a sign-in attempts a provider that is not enabled for the environment
- **THEN** the attempt is refused with a stable error and no session is issued

### Requirement: Magic-link sign-in
An environment MAY enable passwordless sign-in by emailed link. A link SHALL be single-use, bound to the requesting environment and email, expire within a bounded time, and MUST NOT reveal whether the address is registered.

#### Scenario: A user signs in by magic link
- **WHEN** an application user requests a link and opens it before it expires
- **THEN** a session is issued once and the link is spent

### Requirement: Environment email templates
An authorized member SHALL be able to customize the verification, recovery, invitation, and magic-link emails an environment sends, with a bounded set of safe variables. Templates MUST be validated before saving, MUST NOT permit script or remote content, and a preview MUST render with placeholder data.

#### Scenario: A developer customizes the verification email
- **WHEN** an authorized member saves a verification template using only allowed variables
- **THEN** subsequent verification emails for that environment use it, and the preview shows the rendered result
