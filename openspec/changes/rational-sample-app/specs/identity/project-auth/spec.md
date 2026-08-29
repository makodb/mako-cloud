## ADDED Requirements

### Requirement: Browser origins allowed to call an environment
Each environment SHALL carry an allowlist of browser origins, matched exactly. A browser application served from a listed origin SHALL be able to call that environment's application API — authentication, documents, replication, storage, and function invocation — cross-origin, on the platform's own hostname and on any custom domain serving the environment, with the platform answering preflights and echoing the origin. An origin that is not listed MUST receive no cross-origin response headers. The management, operator, and service APIs MUST never answer cross-origin requests, whatever the environment lists. An empty allowlist means no cross-origin access.

#### Scenario: An application on a listed origin calls its project
- **WHEN** an application served from an origin the environment lists sends a preflighted request to the environment's application API
- **THEN** the preflight and the request succeed with cross-origin headers naming that origin, on the platform's hostname and on a custom domain alike

#### Scenario: An unlisted origin and a management route
- **WHEN** the same request arrives from an origin the environment does not list, or a listed origin calls a management, operator, or service route
- **THEN** no cross-origin headers are returned and the browser blocks the response

## MODIFIED Requirements

### Requirement: Trusted and user-editable metadata
The identity service SHALL distinguish administrator-controlled app metadata from user-editable profile metadata. Only verified token claims and administrator-controlled metadata MAY be used as trusted policy inputs. An application's own trusted code SHALL be able to set a user's administrator-controlled app metadata through a service credential, with a bypass reason and an audit record, so that memberships and roles the application manages become trusted claims on that user's next token; user-editable metadata MUST remain unable to reach app metadata by any path.

#### Scenario: User edits profile metadata
- **WHEN** a user changes their display name or other user-editable metadata
- **THEN** that change does not grant roles or permissions controlled by app metadata

#### Scenario: A function sets app metadata under a service credential
- **WHEN** an edge function presents a valid service credential with a bypass reason and sets a user's app metadata
- **THEN** the metadata is written, an audit record names the function, the reason, and the user, the user's next issued token carries the new claims, and the same request without a service credential or from a browser is refused
