# Document policies

Every collection is default deny. Create, read, update, and delete require a
matching active allow rule, and any matching deny wins. Policies are compiled
against the collection schema into a deterministic, bounded evaluator with no
network, wall-clock, or arbitrary-code access.

## Evaluation context

Rules may use the verified user ID, role, trusted claims, project/environment,
operation, safe request metadata, prior document, and proposed document. They
must not treat user-editable profile metadata as trusted authorization input.

- Create evaluates the proposed state.
- Delete evaluates the prior state.
- Update evaluates both states and rechecks the current revision in the same
  conditional transaction that commits the write.
- Read policy applies consistently to point reads, indexed queries, pull, live
  delivery, readable conflicts, caller-aware edge SDK calls, and authorized
  support access.

Protected document bodies must not appear in denial details, unreadable
conflicts, logs, counts, or index diagnostics.

## Expression language

A rule is one boolean expression over the evaluation context. Paths name
context values: `identity.user_id`, `identity.role`, `identity.email`,
`identity.email_verified`, `claims.<name>`, `request.<name>`, `operation`,
`project_id`, `environment_id`, `collection_id`, `old.<field>`, and
`new.<field>`. Literals are JSON strings,
numbers, `true`, `false`, and `null`. Operators are `==`, `!=`, `<`, `<=`,
`>`, `>=`, `&&`, `||`, `!`, and parentheses. Document fields take their type
from the collection schema and an unknown field is a compile error; trusted
claims are dynamic because the schema does not describe them, so a claim
compares with any operand and resolves to `null` when absent.

### Scoping a document to an address

`identity.email` is the address the caller's session authenticated as,
lower-cased, and `null` for a caller that has none. `identity.email_verified`
says whether the environment has confirmed that caller controls it.

They are separate because they answer different questions, and a rule that
hands a document to an address needs both:

```
old.invitee_email == identity.email && identity.email_verified
```

Without the second term the rule hands the document to whoever registered the
address first, which anyone may do in an environment that does not require
verification. Both are verified token claims, set when the session was issued
and not editable by the user; profile metadata still cannot reach them.

### Indexing trusted claims

`claims.<name>[<expression>]` looks a trusted claim up by a value computed at
evaluation time, so a rule can select the claim entry that belongs to the
document it is deciding on. Indexes chain: `claims.a[new.x][new.y]`.

- Only `claims.*` paths may be indexed. Indexing `identity`, `request`, `old`,
  or `new` is the compile error `index_not_allowed` ("only trusted claims may
  be indexed by a value"), addressed to the indexed path.
- The index expression must be a string, a number, or another trusted claim.
  A boolean, `null`, array, or object index is the compile error
  `index_type_invalid`, addressed to the index expression. A missing `]` is a
  `syntax_error` ("expected closing bracket").
- An object indexed by a string yields the member and an array indexed by a
  non-negative integer yields the element. Everything else yields `null`: an
  absent member, an out-of-range, negative, or fractional position, and a
  claim that is a string, number, boolean, or `null`. Indexing never fails
  evaluation, so a missing membership simply fails the comparison it feeds and
  the rule does not match.
- The result is dynamic and compares like any other claim, including
  `!= null` to test that an entry exists.
- Each index costs one node of the bounded evaluation budget, like a path, and
  its index expression is evaluated once.

An application that keeps each member's role per household in the trusted
claims as `households: { "<household_id>": "owner" | "editor" | "viewer" }`
can allow a create when the caller holds a writing role for the household the
document belongs to:

```text
(claims.households[new.household_id] == "owner" || claims.households[new.household_id] == "editor")
```

and allow a read or delete to any member with
`claims.households[old.household_id] != null`. Membership lives in the claims
the token carries, so a change takes effect on the next token and advances the
authorization epoch like any other trusted-claim change.

## Visibility and local data

A visible-to-hidden document change sends a synthetic tombstone containing only
the safe replication identity; hidden-to-visible sends the new state. Policy or
trusted-claim changes increment authorization epochs. The RxDB client then
pauses, securely clears affected replicated state, notifies the application,
and starts a new replication generation before rendering data again.

## Policy lifecycle

Create an immutable draft, validate syntax/types/cost against the active schema,
run representative examples, and atomically activate the complete version.
Activation advances the environment authorization epoch exactly once. Failed
validation or activation leaves the current policy unchanged.

Rollback selects a previously validated immutable version through the normal
audited action. It is an activation, so it also advances the authorization
epoch and requires client resets where visibility may have changed. Never edit
an active policy version in place.

## Privileged access

The default edge document client uses the caller's policies. Bypass requires an
explicit scoped service credential or time-bounded operator grant, the exact
tenant/collection/operation, a reason, and a successful durable audit append.
Audit failure prevents the bypass.

## Tested evidence

Run `npm run test:policy-security`. The suite covers differential decisions,
old/new visibility, epoch invalidation, conflict non-disclosure, privileged
bypass, and caller-aware edge access. See
[policy qualification](policy-security-qualification.md) and the
[policy failure runbook](runbooks/policy-evaluation-failures.md).
