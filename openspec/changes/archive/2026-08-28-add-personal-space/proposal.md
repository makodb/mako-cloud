## Why

Every project belongs to a team, so a developer who only wants a project of their own must first invent a team to hold it. GitHub's model is what developers expect: work of your own lives under you; teams exist when there is a team.

## What Changes

- Every developer gets a personal space on first use: an implicit team of exactly one member, created the first time they create a project without naming a team, with a deterministic id so repeated creation is idempotent. It is marked `kind: personal` and is otherwise a team internally — billing, plan limits, credits, audit, operator tooling, and log collection keep working unchanged, and an individual project's bill shows under the developer's personal space exactly as GitHub bills a personal account.
- `POST /v1/projects` accepts a missing `teamId` and creates the project in the caller's personal space. Teams carry `kind` on the wire.
- A personal space refuses what makes no sense for one person: invitations, role changes, member removal, and deletion answer with a clear conflict. Renaming is allowed.
- The console shows the personal space as "Your projects" ahead of the team list, lets a developer create a project without choosing a team, and hides member management for it.

**Deliberately excluded:** converting a personal space into a team, transferring projects between owners, and account deletion cascading through the personal space.

## Capabilities

- `cloud/control-plane` (modified): developer account and team management gains the personal space.
