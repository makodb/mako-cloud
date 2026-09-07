import { cn } from "@mako-cloud/ui";
import type { ReactNode } from "react";

import {
  type CalendarCell,
  monthGrid,
  type WeekStart,
  weekdayLabels,
} from "../../selectors/calendar.js";
import { readoutLabel } from "./layout.js";

/**
 * A month as a grid of days, whole weeks, with the screen deciding what a day
 * holds. It is markup rather than SVG because a day's contents are text and
 * links that must wrap, reflow, and be tabbed to like any other -- and a
 * table rather than a styled grid because a calendar is one: seven columns
 * with headings, which is what a screen reader should be told.
 *
 * The fixed layout shares the width out evenly whatever a day holds, so a
 * busy day widens nothing; the padding days of the neighbouring months are
 * drawn quieter, and today's number sits on a filled disc.
 */

export interface MonthCalendarProps {
  /** `YYYY-MM`. */
  readonly month: string;
  readonly weekStart?: WeekStart;
  /** An ISO date; the matching cell is marked as today. */
  readonly today?: string;
  readonly renderDay: (cell: CalendarCell) => ReactNode;
  readonly ariaLabel?: string;
}

export function MonthCalendar({
  month,
  weekStart = 0,
  today,
  renderDay,
  ariaLabel,
}: MonthCalendarProps) {
  const weeks = monthGrid(month, weekStart);
  const headings = weekdayLabels(weekStart);
  return (
    <div className="overflow-hidden rounded-xl border bg-card text-card-foreground shadow-xs">
      <table
        className="w-full table-fixed border-collapse text-sm"
        aria-label={ariaLabel ?? `Calendar for ${readoutLabel(month)}`}
        data-month={month}
      >
        <thead>
          <tr>
            {headings.map((heading) => (
              <th
                key={heading}
                scope="col"
                className="border-b bg-muted/40 px-2.5 py-2 text-left text-xs font-semibold tracking-wider text-muted-foreground uppercase"
              >
                {heading}
              </th>
            ))}
          </tr>
        </thead>
        <tbody>
          {weeks.map((week) => (
            <tr key={week[0]?.date ?? ""}>
              {week.map((cell) => {
                const isToday = cell.date === today;
                return (
                  <td
                    key={cell.date}
                    className={cn(
                      "h-24 border-t border-l p-2 align-top first:border-l-0",
                      !cell.inMonth && "bg-muted/30 text-muted-foreground",
                      isToday && "bg-accent/40",
                    )}
                    data-date={cell.date}
                    data-outside={cell.inMonth ? undefined : "true"}
                    aria-current={isToday ? "date" : undefined}
                  >
                    <span
                      className={cn(
                        "inline-flex size-6 items-center justify-center rounded-full text-xs font-medium",
                        isToday && "bg-primary font-semibold text-primary-foreground",
                      )}
                    >
                      <span className="sr-only">{readoutLabel(cell.date)}</span>
                      <span aria-hidden="true">{Number(cell.date.slice(8, 10))}</span>
                    </span>
                    <div className="mt-1 grid gap-0.5">{renderDay(cell)}</div>
                  </td>
                );
              })}
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  );
}
