## MODIFIED Requirements

### Requirement: Operator sessions are isolated, cookie-backed, and revocable
Successful password verification SHALL create an opaque operator session whose credential is stored only in a `Secure`, `HttpOnly`, `SameSite=Strict` cookie with `Path=/v1`, the common prefix of the operator authentication and operator API routes. The server SHALL store only a protected digest of the credential, bind the session to the operator identity, permissions, authorization epoch, issuance time, last verification time, and absolute expiry, and limit the absolute lifetime to at most one hour. Developer, wait-list, application-user, expired, revoked, malformed, and wrong-audience credentials MUST NOT authorize operator routes.

#### Scenario: Operator route accepts a current operator session
- **WHEN** a request carries a current operator cookie with the required permission and current authorization epoch
- **THEN** the operator route authorizes the request as the bound operator identity and records that identity in its audit context

#### Scenario: Developer session cannot cross the boundary
- **WHEN** a developer or wait-list session is presented to an operator route
- **THEN** the route rejects it without treating the developer identity as an operator

#### Scenario: Script cannot read the operator credential
- **WHEN** the console signs in and runs ordinary client-side code
- **THEN** the raw operator credential is unavailable to JavaScript and is sent only by the browser to same-origin paths under `/v1`

#### Scenario: Expired or revoked session fails closed
- **WHEN** an operator session is expired, revoked, has a stale authorization epoch, or refers to removed permissions
- **THEN** every operator route rejects it and the session inspection operation reports no active operator session
