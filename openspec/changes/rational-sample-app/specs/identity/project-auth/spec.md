## MODIFIED Requirements

### Requirement: Trusted and user-editable metadata
The identity service SHALL distinguish administrator-controlled app metadata from user-editable profile metadata. Only verified token claims and administrator-controlled metadata MAY be used as trusted policy inputs. An application's own trusted code SHALL be able to set a user's administrator-controlled app metadata through a service credential, with a bypass reason and an audit record, so that memberships and roles the application manages become trusted claims on that user's next token; user-editable metadata MUST remain unable to reach app metadata by any path.

#### Scenario: User edits profile metadata
- **WHEN** a user changes their display name or other user-editable metadata
- **THEN** that change does not grant roles or permissions controlled by app metadata

#### Scenario: A function sets app metadata under a service credential
- **WHEN** an edge function presents a valid service credential with a bypass reason and sets a user's app metadata
- **THEN** the metadata is written, an audit record names the function, the reason, and the user, the user's next issued token carries the new claims, and the same request without a service credential or from a browser is refused
