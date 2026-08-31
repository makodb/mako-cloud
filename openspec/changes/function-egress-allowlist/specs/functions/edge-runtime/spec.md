## ADDED Requirements

### Requirement: Declared outbound egress
A function deployment SHALL be able to declare a bounded list of external hosts the function may reach over HTTPS, and the runtime SHALL enforce that list at the worker permission boundary: a declared host is reachable, and any other destination — another host, another port, a raw socket, a WebSocket, or a DNS lookup outside the list — is refused by the sandbox, not by convention. A deployment that declares nothing MUST keep today's behavior: the worker reaches the platform API origin and nothing else. The declaration names DNS hosts only, and the platform MUST NOT accept or advertise an egress bound it cannot enforce, such as a per-invocation outbound request count.

Declarations MUST be validated fail-closed at deployment time: an IP literal, a port, a wildcard, a name in the families that resolve inside the platform rather than on the public internet (`localhost`, `metadata`, and everything under `.internal`, `.local`, `.localhost`, and `.arpa`), or a list beyond the documented size cap SHALL be refused with an error naming the offending entry before any version is created.

#### Scenario: A declared host is reachable and an undeclared one is not
- **WHEN** a deployed function whose deployment declares one external HTTPS host fetches that host and then any other destination
- **THEN** the declared fetch proceeds, the undeclared fetch is refused at the runtime boundary with a diagnostic that names no secret values, and the platform SDK's own calls keep working

#### Scenario: An undeclared deployment stays deny-all
- **WHEN** a function deployed with no egress declaration attempts any outbound request beyond the platform API origin
- **THEN** the request is refused exactly as before this capability existed

#### Scenario: An invalid declaration is refused before deployment
- **WHEN** a developer deploys a function declaring an IP literal, a port, a wildcard, a platform-internal name, or more hosts than the documented cap
- **THEN** the deployment is refused with an error naming the offending entry, and no function version is created

#### Scenario: Local serving honours the same declaration
- **WHEN** a developer serves a function locally with the same egress declaration
- **THEN** the local worker grants exactly the declared hosts and refuses others, so a function that runs locally is not one the hosted sandbox will refuse

#### Scenario: The declaration is reviewable and bodies are not logged
- **WHEN** a developer inspects a function whose deployment declares egress hosts
- **THEN** the declared hosts appear in the configuration the API and CLI return, and no outbound request or response body, header, or secret value appears in any log
