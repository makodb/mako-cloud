## Why

The requirement "Rotating refresh sessions" of `identity/project-auth` says
that reuse of an invalidated predecessor triggers replay protection and
mentions an "allowed concurrency grace window", but leaves three things open
that the User Book and the code settle:

- how long the window is and how long a family lives. The User Book says "a
  second spend outside a five-second concurrency grace window is a replay and
  revokes the whole refresh family. A refresh family lives at most thirty
  days";
- which consumed credential the window belongs to. Since the fix of
  2026-11-22 the session store gives it to the credential consumed last only;
  a credential consumed earlier is a replay at once;
- what happens to a credential no family issued. It is refused and nothing is
  revoked.

## What Changes

The requirement states the five-second window, the thirty-day family
lifetime, that replay protection covers every earlier credential of the
family, that the window belongs to the last consumed credential alone, and
that an unknown credential revokes nothing. Three scenarios are added:
"Refresh races its own rotation", "An older consumed credential is
presented", and "An unknown credential is presented".

No code changes: the session store already behaves this way and has a test
for each scenario.

## Capabilities

### New Capabilities

### Modified Capabilities

- `identity/project-auth`: the refresh replay rules are stated in full.

## Impact

- `openspec/specs/identity/project-auth/spec.md`
- `docs/requirements-traceability.md`: rows PA-08 to PA-10 are new; the former
  PA-08 to PA-17 become PA-11 to PA-20.
