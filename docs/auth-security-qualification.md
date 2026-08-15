# Authentication security qualification

Run `npm run test:auth-security` before a release and after changing password policy, tokens, signing keys, sessions, credentials, gateway verification, or client refresh behavior.

The command covers:

- configurable Argon2id password hashing and automatic parameter upgrades;
- enumeration-safe sign-up, sign-in, verification, and password recovery;
- required JWT claims and signature, issuer, audience, tenant, expiry, session, and authorization-epoch verification;
- signing-key encryption, JWKS overlap rotation, and retirement;
- hashed single-use refresh rotation, bounded concurrency grace, family replay detection, and revocation;
- ordered user/session disable, sign-out, deletion, and gateway revocation-cache invalidation;
- one-time public/service credential display, scoping, overlap rotation, and cross-project rejection;
- automatic RxDB-client token refresh and transition to authentication-required when refresh is revoked.

## Latest qualification

The 2026-08-07 local run passed the identity, gateway, and control-plane Rust
suites and all 14 RxDB client tests. Test fixtures use separate
project/environment identities and verify that failures retain stable,
non-enumerating responses without raw credential material.
