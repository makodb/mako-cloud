## Context

The CLI package is already named `@mako-cloud/cli`, and its credential directory and package scope already use `mako-cloud`. The executable map, generated messages, tests, documentation, and example automation still use `mako`. See `proposal.md` for the reason to change it.

The repository also contains many internal names beginning with `mako`, including Rust crates, service executables, protocol headers, environment variables, and deployment scripts. Those identifiers describe internal components or stable configuration contracts rather than the developer CLI.

## Goals / Non-Goals

**Goals:**

- Make `mako-cloud` the only executable installed by `@mako-cloud/cli`.
- Keep every user-visible CLI invocation consistent across code, tests, specifications, documentation, examples, and executable scripts.
- Detect regressions that reintroduce the old developer executable name.

**Non-Goals:**

- Rename internal services, Rust crates, environment variables, HTTP headers, configuration directories, or npm packages.
- Provide a compatibility executable or deprecation period for `mako`.

## Decisions

The package `bin` map will contain only `mako-cloud`. An alias would preserve the ambiguity that prompted the change, and alpha releases do not need a compatibility window.

CLI output will use one exported command-name constant rather than repeating the executable name in the registry and command implementations. This makes generated help and recovery text consistent. Tests will verify the package manifest and scan current user-facing sources for old invocations. A repository scan is preferable to relying only on selected output assertions because command examples also live in prose, scripts, Rust diagnostics, and comments that guide future changes.

The replacement will target developer CLI invocations based on known top-level commands. It will leave component names such as `mako-release`, `mako-data-plane`, `x-mako-*`, and `MAKO_*` intact. Historical OpenSpec archives will remain immutable; the current specification and active change artifacts will use the new name.

## Risks / Trade-offs

- [Existing alpha scripts stop finding `mako`] -> The documented migration is a direct command replacement, and no persisted data or configuration changes.
- [A broad text replacement renames internal components] -> Restrict edits to known CLI command forms and inspect the remaining matches before validation.
- [An old invocation returns later] -> Add a focused automated test that checks the executable map and current source and documentation files.

## Migration Plan

Release the updated package, code, and documentation together. Developers replace `mako ...` with `mako-cloud ...`; credentials remain in the existing `mako-cloud` configuration directory. Rollback restores the previous package and documentation because the change has no data migration.
