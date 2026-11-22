## ADDED Requirements

### Requirement: Proof of an address activates only a pending user
A proof that the person signing in owns a user's address (a redeemed email-verification link, a redeemed magic link, a redeemed password link, or an enabled provider's sign-in whose verified email matches the user) SHALL make a user who is pending verification active, and SHALL leave an active user active. It MUST NOT change a user an administrator disabled or deleted: such a user stays as the administrator left it until an administrator restores it, the proof is refused in the way an unknown or spent proof is, and no session is issued. A session MUST be issued only to an active user, so a flow that ends in a session SHALL activate a pending user before it hands out anything that can be exchanged for one.

#### Scenario: A pending user proves the address
- **WHEN** a user who is pending verification redeems the mailed verification link, or a magic link sent to the address
- **THEN** the user becomes active, and the link cannot be redeemed again

#### Scenario: A provider sign-in matches a pending user
- **WHEN** an enabled provider's sign-in with a verified email matches a user who is pending verification, whether by the email or by a provider identity linked earlier
- **THEN** the user becomes active before the one-time code is issued, and exchanging the code returns a session

#### Scenario: A disabled or deleted user presents a proof
- **WHEN** a user an administrator disabled or deleted redeems a verification link that is still within its lifetime, or signs in through a provider
- **THEN** the proof is refused, no session is issued, and the user keeps the status the administrator gave it
