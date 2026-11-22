## 1. Specifications

- [x] 1.1 `cloud/developer-console`: rewrite "Navigation shell" with the four groups and twenty destinations of the User Book, the permission-following navigation, and the narrow-screen "Go to" control.
- [x] 1.2 `cloud/developer-database-workspace`: use the same four group names in "Grouped navigation and isolated environment state".

## 2. Evidence that the console already meets them

- [x] 2.1 `apps/console/src/developer-workspace.tsx` defines the groups Database, Application, Observe and Configure with these twenty destination ids, and the running console shows them (looked at on 2026-11-21 with a Playwright screenshot of the environment sidebar).
- [x] 2.2 `apps/console/test-e2e/console-shell.spec.ts::a deep link opens inside the shell with its context and every destination` passes.

## 3. Validation

- [x] 3.1 Run `openspec validate name-console-navigation-groups --strict --no-interactive`, `openspec validate --all --strict --no-interactive`, `npm run validate:traceability` and `npm run validate:docs`.
