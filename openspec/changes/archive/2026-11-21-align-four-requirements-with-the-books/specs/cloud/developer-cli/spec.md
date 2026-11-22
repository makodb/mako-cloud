## MODIFIED Requirements

### Requirement: Destructive commands are confirmed
Commands that delete, suspend, retire, revoke, rotate a credential or secret, activate a policy, promote or roll back a deployment, restore a backup, cancel a data job, or mutate a document SHALL require either `--yes` or, on a terminal, the resource's identifier typed back, before anything is sent to the API, and SHALL refuse with the usage exit code without either.

#### Scenario: Deletion without confirmation
- **WHEN** a developer runs a delete command without `--yes` and without a terminal
- **THEN** the command refuses with the usage exit code and nothing is sent to the API
