## ADDED Requirements

### Requirement: Rational is presented through the design system
Every Rational screen — shell, sign-in, dashboard, accounts, transactions, budget, cash flow, recurring, goals, investments, and settings — SHALL render its controls, tables, dialogs, menus, and charts from the shared kit; money SHALL be shown in tabular figures with income and spending told apart by sign and colour; the member's theme preference SHALL be kept per device and SHALL drive the kit's palette on every screen; and the browser suites SHALL keep passing with the same role- and text-based selectors, so the re-skin changes how Rational looks and not what it does.

#### Scenario: The theme preference dresses every screen
- **WHEN** a member switches Rational to dark and moves between screens or reloads
- **THEN** the shell and every screen render with the dark palette, and the choice is remembered on that device

#### Scenario: A money table reads at a glance
- **WHEN** transactions are listed
- **THEN** amounts are right-aligned in tabular figures, spending and income are distinguished by sign and colour, and each row's actions come from the kit

#### Scenario: The parity measure is unchanged by the re-skin
- **WHEN** the re-skinned Rational is validated
- **THEN** every browser scenario passes and the parity matrix reports the same coverage as before
