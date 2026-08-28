## 1. Model and service

- [x] 1.1 Team records carry a kind (team or personal), defaulting to team for every record stored before the field existed.
- [x] 1.2 The team service ensures a developer's personal space: deterministic id, created on first use with the developer as sole owner, idempotent thereafter.
- [x] 1.3 Invitations, role changes, member removal, and deletion refuse a personal space with a distinct conflict.

## 2. Contract and surfaces

- [x] 2.1 `POST /v1/projects` without `teamId` creates the project in the caller's personal space; `Team.kind` is on the wire; OpenAPI and generated types updated.
- [ ] 2.2 The console shows "Your projects" first, creates individual projects without a team, and hides member management for the personal space.

## 3. Proof

- [x] 3.1 Model test: old records load as teams; personal records report their kind.
- [x] 3.2 Service tests: first use creates the space and owner membership; second use returns the same space; the guarded operations refuse it.
- [x] 3.3 End-to-end smoke: a developer creates an individual project against the real services, sees the personal space in their team list, and is refused when inviting to it.
- [ ] 3.4 Console e2e: an individual project is created from "Your projects".
- [ ] 3.5 Traceability rows for the new scenarios at archive.
