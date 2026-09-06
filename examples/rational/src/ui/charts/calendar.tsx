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
    <table
      className="month-calendar"
      aria-label={ariaLabel ?? `Calendar for ${readoutLabel(month)}`}
      data-month={month}
    >
      <thead>
        <tr>
          {headings.map((heading) => (
            <th key={heading} scope="col" className="calendar-weekday">
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
              const classes = ["calendar-cell"];
              if (!cell.inMonth) classes.push("outside");
              if (isToday) classes.push("today");
              return (
                <td
                  key={cell.date}
                  className={classes.join(" ")}
                  data-date={cell.date}
                  aria-current={isToday ? "date" : undefined}
                >
                  <span className="calendar-day-number">
                    <span className="visually-hidden">{readoutLabel(cell.date)}</span>
                    <span aria-hidden="true">{Number(cell.date.slice(8, 10))}</span>
                  </span>
                  <div className="calendar-day-body">{renderDay(cell)}</div>
                </td>
              );
            })}
          </tr>
        ))}
      </tbody>
    </table>
  );
}
