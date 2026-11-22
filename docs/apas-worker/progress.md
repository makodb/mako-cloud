# APAS worker progress

Last updated 2026-11-21, round 1, on Shuais-MacBook-Air-2.local.

## The three layers

- Whitepaper: `docs/user-book.md` and `docs/dev-book.md` (the two books the README names). Read-only for the worker. See the first report entry.
- Specs: `openspec/specs/<area>/<capability>/spec.md`, 29 capabilities, and the open changes under `openspec/changes/`.
- Implementation: the Cargo workspace (`crates/*`, `services/*`), the npm workspace (`packages/*`, `apps/*`, `examples/*`), `api/openapi/`, `scripts/`, `infra/`.

## What round 1 did

- Synced with `origin/main` (already in step), read both books in full, and ran a read-only comparison of whitepaper, specs and code across every capability area. Its 208 leads are in `docs/apas-worker/backlog.md`; none is verified yet.
- Ran the checks. Everything passed except two CLI unit tests in `packages/cli/test/serve.test.mjs`, which failed on any host with no `docker` or `podman` on `PATH` although they test input validation only.
- Step (code layer, requirement "Local development parity" in `openspec/specs/functions/edge-runtime/spec.md`): those two tests now name the container engine as their neighbours do, and a new test covers engine discovery itself with a `PATH` it controls (no engine is refused by name, a named engine needs no lookup, `podman` alone is found, `docker` is preferred when both exist).
- Started the report, these notes, `verus.md` and the backlog.

## What is left, in the order I would do it

1. Spec contradicts the whitepaper: `openspec/specs/operations/public-beta-environment/spec.md` still gates admission on backup age, cost and a 30-day window. The finished change `benchmark-only-beta-gate` fixes it and archives cleanly in a scratch copy. Two things belong to the same step so that `openspec validate --all --strict --no-interactive` stays green: add a delta for `identity/developer-registration` (its requirement "Registration exposure is observable and deny-by-default" refers to the requirement this change removes), and rewrite the `public-beta-environment` delta of `observable-data-plane-and-restore-verification` on top of the new text (both changes modify "Backups and rollback are exercised off the live paths"; the observable delta must keep the scenario "Backups fall behind" and must not make backup state an admission condition).
2. The other 17 "spec may contradict the whitepaper" leads in the backlog, one per round, each after reading the lines again.
3. Code that may not meet a spec requirement: 97 leads. The high-severity ones first (backlog ids 0, 1, 18, 19, 36, 37, 44, 45, 49, 50, 54, 59, 69, 70, 72, 90, 91, 92, 93, 134, 150), foundations before what builds on them.
4. Missing specs: archive `resumable-streaming-exports` (its deltas are not in the main specs yet; it needs traceability rows CL-14 and CL-15 in the same commit or `npm run validate:traceability` fails), then the "whitepaper statement with no spec requirement" and "implemented behaviour with no spec requirement" leads.
5. Missing code: the open change `observable-data-plane-and-restore-verification` (0 of 26 tasks). Its task list names `infra/local/prometheus.yml` (the file is `prometheus.yaml`) and `validate:alert-metric-producers` (the script is `validate:alert-metrics`); fix those in the change first. Start with task group 1 (receipt probe) and group 3 (data-plane `/metrics`).
6. Housekeeping archives of finished changes whose deltas are already in the main specs: `function-egress-allowlist`, `rename-cli-to-mako-cloud`, `redesign-developer-database-console` (each archives as a no-op move in a scratch copy), and `rational-sample-app`. The last one aborts in `openspec archive` because eight of its ADDED requirements already exist in the main spec in the longer form the archived `rational-monarch-parity` change gave them. The worker may not use `--skip-specs`, so its deltas must first be rewritten to match the main spec.
7. Extend `scripts/validate-requirements-traceability.js` and the matrix to the eight capabilities it does not index (the Dev Book says the matrix maps every spec scenario).

## Blocked

- Every Verus step: Verus is not available on this machine. Waits for the report entry of 2026-11-21.
- Tasks that need a server or an account (deploying to the beta VM, converging Ansible roles, watching live alerts, hosted qualification) are never done by the worker. They stay unticked.
- Tasks that edit `docs/user-book.md` or `docs/dev-book.md` (for example task 6.1 of the observable change) wait for the person; the worker proposes the wording in the report.

## What the next round must know

- Checks, as CI names them: `cargo fmt --all -- --check`; `cargo clippy --workspace --all-targets --all-features -- -D warnings`; `cargo test --workspace --lib --bins`; `cargo build --workspace --bins` then `cargo test --workspace --tests`; `npm run format:check`, `lint`, `typecheck`, `test:unit`, `test:integration`; the validators `generate:api:check`, `validate:docs`, `validate:traceability`, `validate:release-gates`, `validate:rollback`, `validate:no-collection`, `validate:rational-parity`, `validate:ui-kit`, `validate:observability`, `validate:alert-metrics`; and `openspec validate --all --strict --no-interactive`.
- This checkout had no `node_modules` or `target`; round 1 ran `npm ci` and built with `CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0 CARGO_INCREMENTAL=0` to keep `target/` small (3.1 GB; the disk has about 33 GB free). Keep those variables so the cache is reused.
- Shuais-MacBook-Air-2.local lacks Caddy, Python's Jinja2, Docker, Podman, `ansible-lint` and Verus. Because of that, `cargo test -p mako-smoke --test explorer_proxy` fails here at "render Caddy" (the Dev Book says that suite requires Caddy and Jinja2), the edge-runtime suites skip, and `ansible-lint` cannot run. These are limits of this machine, not failures of the code; all other Rust integration targets pass (769 tests).
- `.apas` is modified by the APAS tool, not by the worker. Leave it uncommitted.
- `npm run validate:docs` checks every Markdown file under `docs/`, including these notes: keep relative links valid or use none.
- A step that changes what the console or a sample app shows needs a look at the running page in the same round (browser tools are available in this session; the console runs with `npm run dev --workspace @mako-cloud/console`).
