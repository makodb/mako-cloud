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

The Dev Book's newer text (the release-gates section, the admission paragraph and the public-beta runbook) describes a benchmark-only beta gate in which backup state never affects admission. Several older sentences still say the opposite. The worker reads the newer text as the design, because it is dated, it is repeated in three places, and `docs/release-gates.json` and the admission guard agree with it. The next spec step (archiving the `benchmark-only-beta-gate` change so that `openspec/specs/operations/public-beta-environment` stops gating admission on backup age, cost and a 30-day window) depends on that reading. Answer if it is wrong.

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
