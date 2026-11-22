## 1. Specification

- [x] 1.1 Rewrite "Rotating refresh sessions" in `identity/project-auth` with the five-second window, the thirty-day family lifetime, replay protection for every earlier credential, and the refusal of unknown credentials; add the three scenarios.

## 2. Evidence that the code already meets it

- [x] 2.1 `crates/mako-identity/src/session_store.rs::refresh_state_persists_rotates_and_revokes_on_replay` covers the race inside the window and the replay outside it.
- [x] 2.2 `crates/mako-identity/src/session_store.rs::an_older_consumed_credential_is_a_replay_even_inside_the_grace_window` and `::a_credential_consumed_two_rotations_ago_revokes_the_family` cover an older consumed credential.
- [x] 2.3 `crates/mako-identity/src/session_store.rs::a_credential_no_family_issued_is_invalid_and_revokes_nothing` covers an unknown credential.

## 3. Traceability and validation

- [x] 3.1 Add rows PA-08 to PA-10 to `docs/requirements-traceability.md` and renumber the rows after them.
- [x] 3.2 Run `openspec validate state-refresh-replay-rules --strict --no-interactive`, `openspec validate --all --strict --no-interactive`, `npm run validate:traceability` and `npm run validate:docs`.
