## Context

`mako-public-preview-admission` runs every sixty seconds. It verifies a retained approval statically, then re-verifies live conditions, and on any failure selects the source-restricted `pre_gate` configuration. Two separate checks tied that to the deployed release:

- `verify_dynamic` compared the release symlinked at `/opt/mako/current` against the approval's `releaseDigest`, failing with "selected release changed".
- `verify_static` compared every one of `planHash`, `releaseDigest`, and `blockerDigest` between the approval and `/etc/mako/public-preview-context.json`, which Ansible renders from `mako_release_digest`.

Removing only the first would have looked like a fix and lasted until the next converge, when the context file would be re-rendered with the new release and the static comparison would fail instead. Both had to go.

## Goals / Non-Goals

- **Goal:** deploying a release does not close public admission.
- **Goal:** everything that is genuinely about safety still closes it.
- **Non-Goal:** removing the stickiness that requires a fresh acceptance after a fall-closed. That is an operator noticing something went wrong, which is worth keeping.
- **Non-Goal:** making it possible to admit traffic without a risk acceptance at all.

## Decisions

### The approval attests a measurement, not a build

An approval records that the eight non-waivable safeguards were measured, on a named release, against a named plan, with a named blocker set, by a named operator. Treating the named release as a binding turned a provenance record into a lock.

The generator now derives the measured release from the release gate's observation and requires every evidence artifact to agree with it. That keeps the property worth having — the safeguards were measured coherently on one release, not stitched from several runs — without requiring that release to be the one deployed.

### The release digest is retained everywhere it was, and compared nowhere

It stays on the approval, in the context file, and in the guard's evidence output. An operator reading the evidence can still see that admission is running a build whose safeguards were measured on a different one, which is exactly the residual risk being accepted. Deleting the field would have hidden that.

### What this gives up, stated plainly

Nothing now prevents an unqualified release from serving the public. The check that did prevent it is gone, deliberately, because it was closing the site on every deploy and the practical result was worse than the risk it removed.

The honest long-term answer is not this change; it is requalification fast and automatic enough to run inside a deploy. That is real work — four evidence-producing suites, one of them a load test — and it is not what an operator needs at the moment their public site is dark.

## Risks / Trade-offs

- **A broken release can now reach the public.** Mitigated only by the remaining safeguards and by the deploy path itself, which still fences storage, verifies recovery, and refuses to start on failed readiness.
- **The approval's release digest may drift far from the deployed one** over many deploys, making the provenance progressively less meaningful. A future change that reintroduces gating should start by re-measuring rather than by trusting an old acceptance.
