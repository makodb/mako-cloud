# Rollout and verification

PR #19 merged on 2026-10-13 as `2d05fdd5750a16b9514232f30c59d7497a50a4b7`.
All eight CI checks passed on code commit
`6fd8535378e95d219f8f4d700f045a6efd9fd57d` (run `38466833598`).

Release `c6dd088fd1433ce38c586c74e1928ba7b7fca7d5448ed9a987b384c830000d67`
was deployed to the test cloud. Its 929 source files matched the tested commit;
all four services were ready and their installed binary hashes matched the
release. Existing configuration and admission settings were preserved. The
installed `mako-cloud` CLI was updated and rebuilt from merged main.

Focused coverage includes durable worker checkpoints, snapshot loss and cleanup,
pending-upload cleanup, part authorization, a 528 MiB logical export, interrupted
downloads, corrupt saved prefixes, final digests, output ownership, and session
renewal. The real-service CLI workflow and Chromium data-workspace smoke test
passed. Two local serve tests required an unavailable container runtime; the
complete provisioned CI unit suite passed.

The frozen migration source export succeeded with 82,524 unique records,
6,615,522,833 bytes and 394 ordered parts, with zero worker failures. The CLI
was deliberately terminated after 72,702,898 downloaded bytes, then resumed
the same job and output. The complete file matched the cloud manifest
SHA-256, byte count and record count; independent streaming verification
completed at 2026-10-13T23:13:33Z. Automatic developer-session renewal worked
throughout the long-lived process. Control-plane resident memory remained
bounded (about 77–113 MiB in sampled checks as the export grew).

Migration cutover is a separate verification gate: finish sealing the source,
reconcile the read-only destination, verify fresh destination parity, and only
then publish the portfolio head and enable its single writer/dispatcher. The
source remains fenced and the destination head remains unpublished.
