To answer an entry, write a line starting with "Answer:" under it.

# APAS worker report

Entries the worker needs a person to read: questions, assumptions later steps depend on, and proposed changes to the whitepaper. The worker never edits the whitepaper itself.

## 2026-11-21: Which documents the worker treats as the whitepaper

Status: open

The repository has no `docs/whitepaper.md`, `docs/whitepaper/` or `docs/book/`. The README names two books as the project's documentation, so the worker treats both as the whitepaper: `docs/user-book.md` and `docs/dev-book.md`. It reads them and never creates, edits, moves or reformats anything in them.

This has one consequence you should know about. The Dev Book says the opposite about itself: line 7 says the code, the OpenAPI document and the OpenSpec specs win over its prose "and the prose should be fixed in the same change", and line 694 says "When code that this book or the User Book describes changes, change the paragraph in the same commit." `CLAUDE.md` repeats that docs are part of the change. The worker cannot do that. When a spec or code step makes a paragraph of either book stale, the worker writes the proposed wording here and the paragraph stays stale until you change it. Tasks in OpenSpec changes that say "update the Dev Book" or "update the User Book" are left unticked for the same reason.

Where the books disagree with the specs or the code, the worker treats the books as the design and changes the specs or the code, unless the book contradicts itself, in which case it asks here.

If you want a different arrangement (for example: only the User Book is the whitepaper, or the worker may edit the Dev Book), answer below.

## 2026-11-21: Verus is not available on Shuais-MacBook-Air-2.local

Status: open

`verus --version` reports "command not found" on this machine (Shuais-MacBook-Air-2.local), and the project has no verification command in the README, `CLAUDE.md` or `.github/workflows/ci.yml`. The worker does not install tools.

Until this is settled, every Verus step waits: setting Verus up in the project, writing proofs for Rust code, replacing Rust tests with proofs, and judging the existing Rust tests in `docs/apas-worker/verus.md`. New Rust code gets tests in the meantime.

Please either make Verus available on this machine (so that `verus --version` runs), or answer "keep tests" to end the Verus rules for this project.

## 2026-11-21: The Dev Book contradicts itself about the beta gate, backups and admission

Status: open

The Dev Book's newer text (the release-gates section, the admission paragraph and the public-beta runbook) describes a benchmark-only beta gate in which backup state never affects admission. Several older sentences still say the opposite. The worker reads the newer text as the design, because it is dated, it is repeated in three places, and `docs/release-gates.json` and the admission guard agree with it. On that reading the worker archived the `benchmark-only-beta-gate` change on 2026-11-21, so `openspec/specs/operations/public-beta-environment` and `openspec/specs/identity/developer-registration` no longer gate admission on backup age, cost or a 30-day window. Answer if the reading is wrong.

Proposed corrections, all in `docs/dev-book.md`:

1. Line 942 says "Authenticated SQLite backups and signed RocksDB checkpoints are copied to a destination outside the VM failure domain every five minutes." Lines 1028 and 1382 say both backup schedules are disabled on the test preview and have been since 2026-09-25. Proposed: "When `mako_backup_enabled` is true, authenticated SQLite backups and signed RocksDB checkpoints are copied to a destination outside the VM failure domain every five minutes; the test preview currently runs with both schedules disabled."
2. Line 870 says unrestricted admission requires "a passing 30-day gate". Line 1431 says the gate "has no calendar measurement window". Proposed: replace "a passing 30-day gate" with "a passing single-region beta gate".
3. Line 1413 says "Expiry, release or plan drift, blocker drift, failed readiness, stale backups, or TLS/HSTS failure atomically selects `pre_gate`". Line 960 says the acceptance "has no calendar expiry" and "Backup age and backup failures never move admission", and line 1380 says "backup state is not checked". Proposed: "Plan drift, blocker drift, failed readiness, or TLS/HSTS failure atomically selects `pre_gate`". Line 960 also says the acceptance ends when "the exact release or blocker binding changes" while line 1437 says "deploying another release does not change admission"; please say which holds for a risk-accepted preview.
4. Line 1483 says "The single-region beta gate therefore remains blocked until the same release is exercised in its deployment environment", and line 655 says `validate:rollback` checks that "the beta stays blocked". Line 1426 says the single-region beta gate passed for release `b42b2bd4` on 2026-11-11. Proposed: say in both places that the local rollback drills do not by themselves qualify a deployment, and drop "remains blocked" and "the beta stays blocked".
5. Line 868 says "the default and current security posture is no public application listener. Caddy is disabled on VM `124`". Line 960 says "The beta runs in `approved_beta` since 2026-11-11". Proposed: keep "default" and drop "and current", then state the current mode once, in the admission paragraph.

## 2026-11-21: Two tables in the User Book's billing chapter contradict the text around them

Status: open

1. `docs/user-book.md` line 2494 lists "Replication requests | 120 per minute" among the platform rate limits that "every plan carries". Line 2468 says "A removed replication request-rate cap has no minute counter and produces no rate-limit checkpoint". Which holds? If the cap is removed, the worker proposes deleting the table row. Until answered, the worker does not write a spec requirement for a replication request rate limit and leaves the code as it is.
2. Lines 2478 and 2479 put the overage prices in the Free column: "Object storage ... | 1 GiB ($0.02/GiB-month over) | 50 GiB" and "Object egress per month | 5 GiB ($0.09/GiB over) | 250 GiB". Line 2472 says exceeding an included amount on the free plan is refused, and line 1704 says "free: 1 GiB stored, 5 GiB egress, capped; pro: 50 GiB and 250 GiB, overage billed at $0.02/GiB-month and $0.09/GiB". Proposed: move the two prices into the Pro column.

## 2026-11-21: The User Book gives the many-collection stream as GET; the contract defines POST

Status: open

`docs/user-book.md` line 1611 lists `GET /v1/projects/{p}/environments/{e}/replication/stream` for the many-collection live stream. `api/openapi/mako-cloud-v1.yaml` line 3451 defines that path with `post` only (the one-collection stream at line 3494 is `get`). The User Book says at line 9 that the OpenAPI document wins where they disagree, so the worker leaves the code and the contract as they are. Proposed: change the method in line 1611 to `POST` and say that the body names the collections and their cursors.

## 2026-11-21: Does a wait-list decision need a reason?

Status: open

Two specs disagree, and the Dev Book sides with one of them while the code follows the other.

- `docs/dev-book.md` line 1218: "Every decision requires recent password verification, an explicit private reason, idempotency, and confirmation." Line 880 says an operator can "atomically approve or reject one with a private reason and idempotency key".
- `openspec/specs/identity/developer-registration/spec.md` lines 98 to 100: "Approval and rejection MUST require an operator-visible reason, request identifier, and idempotency key".
- `openspec/specs/identity/account-role-lifecycle/spec.md` line 89: "The system SHALL accept individual approval and rejection decisions when the private review reason is omitted, null, empty, or whitespace-only."
- `crates/mako-control-plane/src/developer_workflow.rs` line 1114 validates a reason only when it is not empty, so a decision without a reason is accepted.

Which rule do you want?

1. A reason is required. Then the worker changes the `account-role-lifecycle` requirement and makes the control plane refuse a decision with no reason. The operator console's review form changes with it.
2. A reason is optional. Then the worker changes the `developer-registration` requirement, and you change the two Dev Book sentences to say "an optional private reason".

Until this is answered the worker leaves both specs and the code as they are.

## 2026-11-22: Which request headers does a hosted function receive?

Status: open

Local serve and hosted invocation disagree, and the books do not settle it.

- Hosted: the edge gateway forwards nine request headers and drops the rest (`forwarded_request_headers` in `services/mako-edge-gateway/src/http.rs`): `authorization` (replaced by the verified caller), `accept`, `content-type`, `idempotency-key`, `if-match`, `if-none-match`, `user-agent`, `traceparent`, `tracestate`. A third-party webhook's signature header, a custom header, or a cookie never reaches the function.
- Local: `mako-cloud functions serve` passes every header through except `authorization` and those under `x-mako-` (`packages/cli/runtime/main/index.ts`).
- `docs/user-book.md` line 1714 says "Functions receive the Fetch `Request`/`Response` contract", and line 1876 says a locally served function receives the "headers" and lists regional routing, hosted quotas and production egress as the differences from hosted. `openspec/specs/functions/edge-runtime/spec.md` asks local serve to run "the same request contract" as hosted execution.

Which do you want?

1. Hosted functions receive every caller header except a reserved set (hop-by-hop headers, `host`, `content-length`, `expect`, `authorization`, and everything under `x-mako-`). The worker would then change the gateway and write the reserved set into the spec, and you would list it in the User Book.
2. Hosted functions receive only a fixed list. The worker would then narrow local serve to the same list and write the list into the spec, and you would list it in the User Book.

In either case: should `cookie` reach a function? Every tenant's functions share the platform hostname, and line 2241 says cookies are not part of the exchange, so the worker would withhold it unless you say otherwise.

Until this is answered the worker leaves the gateway and local serve as they are.

## 2026-11-22: The User Book's list of application routes omits the many-collection stream

Status: open

`docs/user-book.md` line 2243 defines application routes as "the ones an application calls with its own session or public key" and then lists "`…/auth/…`, `…/collections/…` (documents and replication), and `…/storage/…`". The many-collection live stream is `…/environments/{e}/replication/stream` (line 1611), which is under none of the three. The definition covers it and `@mako-cloud/rxdb` opens it from the browser, so the worker reads the list as incomplete and will make the data plane answer listed origins on that route (the requirement "Browser origins allowed to call an environment" already asks for replication). Proposed: add "`…/replication/stream` (the many-collection live stream)" to the list in line 2243.

## 2026-11-22: After a RocksDB corruption signal the service now stops itself; should repeated I/O errors do the same?

Status: open

`openspec/specs/storage/production-rocksdb/spec.md` says that when "an integrity check or database operation reports corruption", "the service fails readiness, stops accepting mutations, and directs operators to the documented recovery procedure". The code only counted the error. Since 2026-11-22 the RocksDB adapter does what the scenario says: after the first corruption error from any operation, storage health is unavailable, every write is refused, reads stay open, and `/readyz` of the data plane, the edge gateway and the telemetry service carries `storage_corruption_detected:mutations_refused:see=docs/dev-book.md#runbook-production-rocksdb-incidents`. The state lasts until the process is restarted, which is step 6 of that runbook.

Two things for you:

1. Proposed addition to `docs/dev-book.md` line 1061 ("On a corruption or repeated I/O signal, stop routing and mutations, fence the owner, ..."), which reads as if the operator has to stop mutations by hand. Add after it: "The service does the first part by itself on a corruption signal: storage readiness turns false with a detail that names the RocksDB runbook, every write is refused, and reads stay open, until the process is restarted. Most authenticated requests then answer 503, because they write quota counters and replay guards." The same sentence fits the runbook at line 1351.
2. A question. Line 1061 also names a "repeated I/O signal". The spec scenario names corruption only, and an I/O error can pass (a full disk, a slow device), so the worker left I/O errors as they were: counted, reported to the caller as retryable, and not a reason to fail readiness. Do you want repeated I/O errors to fail readiness and stop writes too? If yes, say how many within what time, and whether the state clears by itself when writes succeed again.

## 2026-11-22: Should proving an address discard a password set before the address was proven?

Status: open

A password sign-up that awaits email verification stores the password its author chose. If somebody else signs up with your address first, their password is on the pending account. When you then prove the address, by a magic link (`docs/user-book.md` line 1196: "A magic link to the same address also proves it and activates the account") or, since 2026-11-22, by a provider sign-in with that verified email, the account becomes active and the earlier password is still valid for it. The person who signed up first can sign in to your account with it.

The books do not say what happens to that password. The worker kept the existing behaviour of magic links for the provider path, so both leave the password in place.

Which do you want?

1. Activation by a magic link or a provider discards a password that was set while the account was pending; the owner sets one through password recovery if they want one. The worker would then change both paths and write the rule into `identity/auth-providers` and `identity/project-auth`.
2. Keep the password. Then please say so in the User Book, since an application that requires email verification probably relies on the opposite.

## 2026-11-22: Two sentences in the User Book about provider sign-in

Status: open

1. `docs/user-book.md` line 1176 says the application user "is matched by provider and subject first, then by verified email, and created when neither matches", and line 1177 lists the refusals. Neither says what happens when the matched user is still pending verification (a password sign-up that has not opened its mail, an invitation, or a magic link nobody redeemed). Since 2026-11-22 the provider's verified email activates such a user, as a magic link does. Proposed addition to line 1176: "A matched user who is still pending verification becomes active: the provider's verified email proves the address."
2. Line 1312 gives `email_not_verified` as an example of the reason a provider refusal carries. The data plane sends `email_unverified` (`services/mako-data-plane/src/auth_provider_http.rs`). Proposed: change the example to `email_unverified`.
