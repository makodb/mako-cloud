## 1. Cloud export contract and worker

- [x] 1.1 Add compatible multipart manifest and authenticated part download contracts.
- [x] 1.2 Persist bounded immutable parts, upload intents and export checkpoints between worker passes.
- [x] 1.3 Handle snapshot loss, cancellation, expiry and final digest verification without mixing generations.

## 2. Clients and regression tests

- [x] 2.1 Implement protected resumable CLI part downloads and safe console handling.
- [x] 2.2 Test worker retries, snapshot loss, bounded payloads, part authorization, cleanup and interrupted/corrupt downloads.
- [x] 2.3 Regenerate API types, document export/resume behavior, validate specs and run required tests, lint and builds.

## 3. Deployment and migration retry

- [x] 3.1 Publish the change, build and deploy a verified cloud release, and update the installed CLI.
- [x] 3.2 Retry the frozen-source export, verify a collection larger than 512 MiB and exercise download resumption; record remaining migration status accurately.
