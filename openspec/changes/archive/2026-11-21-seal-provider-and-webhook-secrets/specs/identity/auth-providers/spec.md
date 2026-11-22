## MODIFIED Requirements

### Requirement: External sign-in providers
An authorized member SHALL be able to enable OAuth and OpenID Connect providers for an environment by supplying each provider's client identifier and client secret, and to disable them. Application users SHALL sign in through an enabled provider and receive the same session as password sign-in, with the provider identity linked to the application user by verified email and provider subject.

A client secret is supplied as a value. It MUST be sealed before it is stored, under a key bound to the project, the environment and the provider name, so that a sealed secret cannot be moved to another environment or another provider. It MUST NOT be returned once saved: a read of the settings SHALL report only whether a secret is installed for each provider. Because the settings are replaced whole, a provider given without a client secret SHALL keep the secret already installed under its name, and a provider that has no installed secret and is given none SHALL be refused before anything is installed. Removing a provider SHALL remove its secret with it.

#### Scenario: An application user signs in with a provider
- **WHEN** an application user completes an enabled provider's flow with a verified email
- **THEN** an application-user session is issued for the matching or newly created user and the sign-in is recorded as an authentication event

#### Scenario: A disabled provider is used
- **WHEN** a sign-in attempts a provider that is not enabled for the environment
- **THEN** the attempt is refused with a stable error and no session is issued

#### Scenario: A client secret is saved and never returned
- **WHEN** an authorized member saves a provider with its client secret
- **THEN** the secret is sealed for that project, environment and provider name before it is stored, it does not open under another provider's name, and a later read of the settings reports only that a secret is installed

#### Scenario: A provider is saved without its client secret
- **WHEN** an authorized member replaces the settings with a provider that names no client secret
- **THEN** the secret already installed under that provider's name is kept, and a provider with no installed secret is refused before anything is installed
