## Purpose

Let applications store and serve files — images, uploads, attachments — next to their documents, governed by the same policies and metered like every other resource.

## ADDED Requirements

### Requirement: Buckets per environment
An authorized member SHALL be able to create, list, configure, and delete buckets in an environment. A bucket SHALL declare whether objects are public or policy-governed, a maximum object size, and allowed content types, and MUST be deleted only when empty or with explicit confirmation of object loss.

#### Scenario: A developer creates a bucket
- **WHEN** an authorized member creates a bucket with a size limit and allowed content types
- **THEN** the bucket is listed for the environment and application requests outside those limits are refused with a stable error

### Requirement: Policy-governed object access
Application users SHALL upload, download, list, and delete objects under a bucket's policies, evaluated against the application-user session exactly as document policies are. Public buckets SHALL serve reads without a session; every write MUST be authorized. Object paths MUST be validated and MUST NOT allow escaping the bucket. Service credentials MAY bypass policies with privileged audit context.

#### Scenario: An application user uploads under policy
- **WHEN** an application user uploads an object to a path their policy allows
- **THEN** the object is stored, its metadata records the owner and content type, and a later download by the same user succeeds

#### Scenario: A read the policy denies
- **WHEN** an application user requests an object their policy does not allow
- **THEN** the request is refused with a stable authorization error and no object bytes are sent

### Requirement: Storage is metered and bounded
Stored object bytes and object egress SHALL be metered per environment on the billing ledger and counted against the plan's allowances; a bucket's object count and total bytes SHALL be reported in the console, and requests beyond a plan's caps SHALL be refused with retry advice.

#### Scenario: Stored files appear on the bill
- **WHEN** an environment holds objects across a billing period
- **THEN** the period's bill lists stored object bytes and egress as line items with quantity, allowance, and amount
