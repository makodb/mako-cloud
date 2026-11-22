## MODIFIED Requirements

### Requirement: A team has a running balance that may be negative
Each team SHALL have one account balance: its credits minus all of its charges, which are the finalized charges of every closed period plus the charges the live month has accrued so far. The balance MUST NOT be clamped: it MUST be permitted to go negative, meaning the team has accrued more use than credit. It MUST be derivable from the retained invoices, the live month's retained usage, and the credit entries that produced it. Every bill response for a team SHALL report that same balance, whether the response shows the live month or a closed period.

#### Scenario: Use accrues beyond any credit
- **WHEN** a team's charges exceed the credits it holds
- **THEN** the balance is reported as negative and the invoices and credits that produced it remain available

#### Scenario: A balance is explained
- **WHEN** an authorized member inspects the balance
- **THEN** each closed period behind it is retrievable as a finalized invoice and the live month derives on read

#### Scenario: An operator grants credit
- **WHEN** an operator applies a credit with a reason and an unused idempotency key
- **THEN** the credit is applied exactly once, the balance moves by that amount, and the action is audited

#### Scenario: A closed period is read during a month with use
- **WHEN** an authorized member reads a closed period's invoice while the live month has accrued charges
- **THEN** the response shows the closed period's charges, and its balance equals the balance the live bill reports
