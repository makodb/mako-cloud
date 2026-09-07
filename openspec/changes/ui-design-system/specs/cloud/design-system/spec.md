## Purpose

One design system for every Mako web surface — the developer console and the Rational sample — so that controls, colour, type, icons, and charts are consistent, accessible, and themed the same way everywhere, and so that a standalone application built on it still builds on its own.

## ADDED Requirements

### Requirement: One kit dresses every web surface
Every Mako web surface SHALL draw its controls, surfaces, typography, icons, and charts from one shared kit. The kit SHALL define its palette, spacing, radius, and type scale as named tokens with a light and a dark value each; an application MUST NOT define its own buttons, fields, menus, dialogs, or chart styles beside the kit's. The kit SHALL ship one typeface, self-hosted, so no page depends on a third-party font service.

#### Scenario: Tokens follow the theme
- **WHEN** the document root carries the dark theme, or carries no theme and the device prefers dark
- **THEN** every token resolves to its dark value, and text on every surface keeps at least the AA contrast ratio against it

#### Scenario: An application adopts the kit with one import
- **WHEN** an application imports the kit's stylesheet and components
- **THEN** the typeface, tokens, base styles, and focus treatment apply to it without further configuration

#### Scenario: A raw control slips into an application
- **WHEN** an application source renders a button, field, select, text area, or dialog element directly instead of the kit's component
- **THEN** the repository's validation refuses it and names the file

### Requirement: Components are operable by everyone
Every interactive component in the kit SHALL be usable by keyboard and labelled for assistive technology: a dialog, sheet, menu, or select that opens MUST take focus, MUST close on Escape and return focus to what opened it, and MUST expose its role and name; a control's focus MUST be visible in both themes.

#### Scenario: A dialog is opened from the keyboard
- **WHEN** a member opens a dialog and presses Escape
- **THEN** focus had moved into the dialog while it was open, the dialog closes, and focus returns to the control that opened it

### Requirement: Charts are consistent and readable
Every chart on a web surface SHALL be drawn by the kit with the palette's series colours, axes with formatted values, and a tooltip naming the point under the pointer; a chart SHALL carry an accessible name describing what it shows, and SHALL not animate when the device asks for reduced motion.

#### Scenario: A series is shown
- **WHEN** a screen renders a time series or a breakdown
- **THEN** the chart names itself to assistive technology, its axes show formatted values, and pointing at a value shows the point's label and formatted amount

### Requirement: A standalone application carries the kit
An application exported from the workspace to stand alone SHALL carry the kit with it — its sources and every dependency the kit needs spelled out — so the exported repository builds and its tests run without the workspace.

#### Scenario: The exported application builds alone
- **WHEN** Rational is exported to its own repository
- **THEN** the export contains the kit's sources and declares its dependencies, and the exported repository's build and browser suite pass with no reference to the workspace
