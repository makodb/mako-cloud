/** Writable enough for stdout and stderr, real or captured in tests. */
export interface OutputSink {
  write(chunk: string): unknown;
}

export interface TableColumn {
  readonly key: string;
  readonly label?: string;
}

/** Renders a scalar for a table cell or a record line; objects become JSON. */
export function formatValue(value: unknown): string {
  if (value === null || value === undefined) return "—";
  if (typeof value === "string") return value;
  if (typeof value === "number" || typeof value === "boolean" || typeof value === "bigint") {
    return String(value);
  }
  return JSON.stringify(value);
}

function columnsFor(rows: readonly Record<string, unknown>[]): TableColumn[] {
  const seen = new Map<string, number>();
  for (const row of rows) {
    for (const [key, value] of Object.entries(row)) {
      if (value !== null && typeof value === "object") continue;
      if (!seen.has(key)) seen.set(key, seen.size);
    }
  }
  return [...seen.keys()].slice(0, 8).map((key) => ({ key }));
}

/** A plain-text table: header, rule, one line per row; empty input says so. */
export function renderTable(
  rows: readonly Record<string, unknown>[],
  columns?: readonly TableColumn[],
): string {
  if (rows.length === 0) return "(none)\n";
  const chosen = columns ?? columnsFor(rows);
  if (chosen.length === 0) return `${rows.map((row) => JSON.stringify(row)).join("\n")}\n`;
  const cells = rows.map((row) => chosen.map((column) => formatValue(row[column.key])));
  const labels = chosen.map((column) => column.label ?? column.key);
  const widths = labels.map((label, index) =>
    Math.max(label.length, ...cells.map((line) => line[index]?.length ?? 0)),
  );
  const pad = (line: readonly string[]) =>
    line
      .map((cell, index) => cell.padEnd(widths[index] ?? 0))
      .join("  ")
      .trimEnd();
  return [pad(labels), widths.map((width) => "-".repeat(width)).join("  "), ...cells.map(pad)]
    .map((line) => `${line}\n`)
    .join("");
}

/** One `key: value` line per field; nested objects are indented JSON. */
export function renderRecord(value: Record<string, unknown>): string {
  const keys = Object.keys(value);
  if (keys.length === 0) return "{}\n";
  const width = Math.max(...keys.map((key) => key.length));
  return keys
    .map((key) => {
      const field = value[key];
      const rendered =
        field !== null && typeof field === "object"
          ? JSON.stringify(field, null, 2).replace(/\n/gu, `\n${" ".repeat(width + 2)}`)
          : formatValue(field);
      return `${key.padEnd(width)}  ${rendered}\n`;
    })
    .join("");
}

/** The delimited block a one-time secret is printed in on a terminal. */
export function renderSecretBlock(label: string, secret: string): string {
  const rule = "-".repeat(Math.max(24, label.length + 20));
  return `${rule}\n${label} (shown once)\n${secret}\n${rule}\n`;
}
