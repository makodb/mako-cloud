## Why

`identity/developer-registration` asks registration to apply a password policy
and to give generic responses, but has no scenario for a password the policy
refuses. That gap is where the code went wrong until commit 8a12d77: such a
password was accepted for a registered address and answered as an outage for
an unknown one. The fix settled what the requirement leaves open, and the same
for password recovery:

- the policy is checked with the rest of the payload, before any identity or
  token is looked up;
- the refusal is an invalid request, the same whoever the address belongs to
  and whatever the token is;
- a refused recovery does not spend the token.

## What Changes

- "Public developer registration is bounded and enumeration-safe" says when
  the policy is checked and gains the scenario "Password does not satisfy the
  policy".
- "Developer credentials and sessions follow hosted security controls" says
  the same for recovery and gains the scenario "Recovery password does not
  satisfy the policy".

No code changes: both are covered by tests added with the fix.

## Capabilities

### New Capabilities

### Modified Capabilities

- `identity/developer-registration`: what happens to a password the policy
  refuses, at registration and at recovery.

## Impact

- `openspec/specs/identity/developer-registration/spec.md`
- `docs/requirements-traceability.md`: new rows DR-04 and DR-14; the former
  DR-04 to DR-12 become DR-05 to DR-13 and the former DR-13 to DR-32 become
  DR-15 to DR-34.
