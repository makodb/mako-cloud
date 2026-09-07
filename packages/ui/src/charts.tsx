import type { ReactNode } from "react";
import {
  Area,
  AreaChart as RechartsAreaChart,
  Bar,
  BarChart as RechartsBarChart,
  CartesianGrid,
  Cell,
  Line,
  LineChart as RechartsLineChart,
  Pie,
  PieChart,
  ResponsiveContainer,
  Tooltip,
  type TooltipPayloadEntry,
  XAxis,
  YAxis,
} from "recharts";

import { cn } from "./lib/utils.js";
import { seriesColor } from "./tokens.js";

/** One row of a chart: the x value under `x`, and a number per series key. */
export type ChartDatum = Record<string, number | string | null | undefined>;

export interface ChartSeries {
  /** The key in each datum. */
  key: string;
  /** What the legend and tooltip call it. */
  label: string;
  /** A CSS colour; the palette's next series colour when absent. */
  color?: string;
}

interface CartesianChartProps {
  data: ReadonlyArray<ChartDatum>;
  /** The key of the x value in each datum. */
  x: string;
  series: ReadonlyArray<ChartSeries>;
  /** What the chart shows, for assistive technology. */
  title: string;
  height?: number;
  formatValue?: (value: number) => string;
  /** Axis ticks, when they should read differently from the tooltip ($12K vs $12,000.00). */
  formatTick?: (value: number) => string;
  formatX?: (value: string | number) => string;
  /** Room for the y-axis labels; widen it for long formatted amounts. */
  yAxisWidth?: number;
  className?: string;
  /** Show the legend even for a single series. */
  legend?: boolean;
}

const identity = (value: string | number) => String(value);
const plain = (value: number) => String(value);

/**
 * Whether a chart may animate: never when the device asks for reduced motion,
 * and never under a browser suite, which marks the root with `data-testing`
 * so what it reads is the finished drawing.
 */
export function motionAllowed(): boolean {
  if (typeof window === "undefined" || typeof document === "undefined") return false;
  if (document.documentElement.dataset.testing !== undefined) return false;
  if (typeof window.matchMedia !== "function") return true;
  return !window.matchMedia("(prefers-reduced-motion: reduce)").matches;
}

/** What Recharts hands a tooltip: whether it is shown, the rows under the pointer, and the x value. */
interface TooltipState {
  active?: boolean | undefined;
  payload?: ReadonlyArray<TooltipPayloadEntry> | undefined;
  label?: unknown;
}

function ChartTooltipContent({
  active,
  payload,
  label,
  formatValue,
  formatX,
  labels,
}: TooltipState & {
  formatValue: (value: number) => string;
  formatX: (value: string | number) => string;
  labels: ReadonlyMap<string, string>;
}) {
  if (!active || payload === undefined || payload.length === 0) return null;
  const heading =
    typeof label === "string" || typeof label === "number" ? formatX(label) : undefined;
  return (
    <div className="min-w-32 rounded-md border bg-popover px-3 py-2 text-xs text-popover-foreground shadow-md">
      {heading !== undefined ? <div className="mb-1 font-medium">{heading}</div> : null}
      <div className="grid gap-1">
        {payload.map((entry) => {
          const key =
            typeof entry.dataKey === "function"
              ? String(entry.name ?? "")
              : String(entry.dataKey ?? entry.name ?? "");
          return (
            <div key={key} className="flex items-center gap-2">
              <span
                aria-hidden="true"
                className="size-2 shrink-0 rounded-[2px]"
                style={{ background: entry.color }}
              />
              <span className="text-muted-foreground">
                {labels.get(key) ?? String(entry.name ?? "")}
              </span>
              <span className="money ml-auto font-medium">
                {formatValue(Number(entry.value ?? 0))}
              </span>
            </div>
          );
        })}
      </div>
    </div>
  );
}

function Legend({ series }: { series: ReadonlyArray<ChartSeries> }) {
  return (
    <ul className="m-0 flex flex-wrap gap-x-4 gap-y-1 p-0 text-xs text-muted-foreground">
      {series.map((entry, index) => (
        <li key={entry.key} className="flex items-center gap-1.5">
          <span
            aria-hidden="true"
            className="size-2 rounded-[2px]"
            style={{ background: entry.color ?? seriesColor(index) }}
          />
          {entry.label}
        </li>
      ))}
    </ul>
  );
}

function Frame({
  title,
  height,
  className,
  legend,
  legendBelow = false,
  children,
}: {
  title: string;
  height: number;
  className?: string | undefined;
  legend?: ReactNode | undefined;
  legendBelow?: boolean;
  children: ReactNode;
}) {
  return (
    <figure
      role="img"
      aria-label={title}
      data-slot="chart"
      className={cn("m-0 grid w-full gap-2", className)}
    >
      {legendBelow ? null : legend}
      <div style={{ height }} className="relative w-full text-xs">
        {children}
      </div>
      {legendBelow ? legend : null}
    </figure>
  );
}

const axisTick = { fill: "var(--muted-foreground)", fontSize: 11 };
const grid = <CartesianGrid vertical={false} stroke="var(--border)" strokeDasharray="0" />;

function labelsOf(series: ReadonlyArray<ChartSeries>): ReadonlyMap<string, string> {
  return new Map(series.map((entry) => [entry.key, entry.label]));
}

/** A value over time, one line per series. */
export function LineChart({
  data,
  x,
  series,
  title,
  height = 240,
  formatValue = plain,
  formatTick = formatValue,
  formatX = identity,
  yAxisWidth = 56,
  className,
  legend,
}: CartesianChartProps) {
  const animate = motionAllowed();
  const labels = labelsOf(series);
  return (
    <Frame
      title={title}
      height={height}
      className={className}
      legend={legend || series.length > 1 ? <Legend series={series} /> : undefined}
    >
      <ResponsiveContainer width="100%" height="100%">
        <RechartsLineChart
          data={data as ChartDatum[]}
          margin={{ top: 8, right: 8, bottom: 0, left: 0 }}
        >
          {grid}
          <XAxis
            dataKey={x}
            tickLine={false}
            axisLine={false}
            tick={axisTick}
            tickFormatter={formatX}
            minTickGap={24}
          />
          <YAxis
            tickLine={false}
            axisLine={false}
            tick={axisTick}
            tickFormatter={formatTick}
            width={yAxisWidth}
          />
          <Tooltip
            cursor={{ stroke: "var(--border)" }}
            content={({ active, payload, label }) => (
              <ChartTooltipContent
                active={active}
                payload={payload}
                label={label}
                formatValue={formatValue}
                formatX={formatX}
                labels={labels}
              />
            )}
          />
          {series.map((entry, index) => (
            <Line
              key={entry.key}
              type="monotone"
              dataKey={entry.key}
              name={entry.label}
              stroke={entry.color ?? seriesColor(index)}
              strokeWidth={2}
              dot={false}
              activeDot={{ r: 4 }}
              isAnimationActive={animate}
            />
          ))}
        </RechartsLineChart>
      </ResponsiveContainer>
    </Frame>
  );
}

/** A value over time with the area under it filled: net worth, a balance. */
export function AreaChart({
  data,
  x,
  series,
  title,
  height = 240,
  formatValue = plain,
  formatTick = formatValue,
  formatX = identity,
  yAxisWidth = 56,
  className,
  legend,
}: CartesianChartProps) {
  const animate = motionAllowed();
  const labels = labelsOf(series);
  return (
    <Frame
      title={title}
      height={height}
      className={className}
      legend={legend || series.length > 1 ? <Legend series={series} /> : undefined}
    >
      <ResponsiveContainer width="100%" height="100%">
        <RechartsAreaChart
          data={data as ChartDatum[]}
          margin={{ top: 8, right: 8, bottom: 0, left: 0 }}
        >
          <defs>
            {series.map((entry, index) => (
              <linearGradient key={entry.key} id={`area-${entry.key}`} x1="0" y1="0" x2="0" y2="1">
                <stop
                  offset="5%"
                  stopColor={entry.color ?? seriesColor(index)}
                  stopOpacity={0.35}
                />
                <stop
                  offset="95%"
                  stopColor={entry.color ?? seriesColor(index)}
                  stopOpacity={0.02}
                />
              </linearGradient>
            ))}
          </defs>
          {grid}
          <XAxis
            dataKey={x}
            tickLine={false}
            axisLine={false}
            tick={axisTick}
            tickFormatter={formatX}
            minTickGap={24}
          />
          <YAxis
            tickLine={false}
            axisLine={false}
            tick={axisTick}
            tickFormatter={formatTick}
            width={yAxisWidth}
          />
          <Tooltip
            cursor={{ stroke: "var(--border)" }}
            content={({ active, payload, label }) => (
              <ChartTooltipContent
                active={active}
                payload={payload}
                label={label}
                formatValue={formatValue}
                formatX={formatX}
                labels={labels}
              />
            )}
          />
          {series.map((entry, index) => (
            <Area
              key={entry.key}
              type="monotone"
              dataKey={entry.key}
              name={entry.label}
              stroke={entry.color ?? seriesColor(index)}
              strokeWidth={2}
              fill={`url(#area-${entry.key})`}
              isAnimationActive={animate}
            />
          ))}
        </RechartsAreaChart>
      </ResponsiveContainer>
    </Frame>
  );
}

/** Amounts per period, one bar per series, side by side or stacked. */
export function BarChart({
  data,
  x,
  series,
  title,
  height = 240,
  formatValue = plain,
  formatTick = formatValue,
  formatX = identity,
  yAxisWidth = 56,
  className,
  legend,
  stacked = false,
}: CartesianChartProps & { stacked?: boolean }) {
  const animate = motionAllowed();
  const labels = labelsOf(series);
  return (
    <Frame
      title={title}
      height={height}
      className={className}
      legend={legend || series.length > 1 ? <Legend series={series} /> : undefined}
    >
      <ResponsiveContainer width="100%" height="100%">
        <RechartsBarChart
          data={data as ChartDatum[]}
          margin={{ top: 8, right: 8, bottom: 0, left: 0 }}
          barGap={2}
        >
          {grid}
          <XAxis
            dataKey={x}
            tickLine={false}
            axisLine={false}
            tick={axisTick}
            tickFormatter={formatX}
            minTickGap={16}
          />
          <YAxis
            tickLine={false}
            axisLine={false}
            tick={axisTick}
            tickFormatter={formatTick}
            width={yAxisWidth}
          />
          <Tooltip
            cursor={{ fill: "var(--accent)", fillOpacity: 0.5 }}
            content={({ active, payload, label }) => (
              <ChartTooltipContent
                active={active}
                payload={payload}
                label={label}
                formatValue={formatValue}
                formatX={formatX}
                labels={labels}
              />
            )}
          />
          {series.map((entry, index) => (
            <Bar
              key={entry.key}
              dataKey={entry.key}
              name={entry.label}
              fill={entry.color ?? seriesColor(index)}
              radius={stacked ? 0 : 3}
              maxBarSize={40}
              isAnimationActive={animate}
              {...(stacked ? { stackId: "stack" } : {})}
            />
          ))}
        </RechartsBarChart>
      </ResponsiveContainer>
    </Frame>
  );
}

export interface DonutSlice {
  name: string;
  value: number;
  color?: string;
  /** A stable handle for the slice, carried as `data-key` on its legend row. */
  key?: string;
}

/** A whole and its parts: spending by category, allocation by asset class. */
export function DonutChart({
  data,
  title,
  height = 220,
  formatValue = plain,
  className,
  center,
  legend = false,
}: {
  data: ReadonlyArray<DonutSlice>;
  title: string;
  height?: number;
  formatValue?: (value: number) => string;
  className?: string;
  /** Text in the hole: the total and what it is. */
  center?: { label: string; value: string };
  /** A row per slice under the chart, with its formatted value. */
  legend?: boolean;
}) {
  const animate = motionAllowed();
  const labels = new Map(data.map((slice) => [slice.name, slice.name]));
  const rows = legend ? (
    <ul className="m-0 grid list-none gap-1 p-0 text-sm">
      {data.map((slice, index) => (
        <li
          key={slice.key ?? slice.name}
          data-key={slice.key ?? slice.name}
          className="flex items-center gap-2"
        >
          <span
            aria-hidden="true"
            className="size-2.5 shrink-0 rounded-[3px]"
            style={{ background: slice.color ?? seriesColor(index) }}
          />
          <span className="truncate">{slice.name}</span>
          <span className="money ml-auto text-muted-foreground">{formatValue(slice.value)}</span>
        </li>
      ))}
    </ul>
  ) : undefined;
  return (
    <Frame title={title} height={height} className={className} legend={rows} legendBelow>
      <ResponsiveContainer width="100%" height="100%">
        <PieChart>
          <Tooltip
            content={({ active, payload, label }) => (
              <ChartTooltipContent
                active={active}
                payload={payload}
                label={label}
                formatValue={formatValue}
                formatX={identity}
                labels={labels}
              />
            )}
          />
          <Pie
            data={data as DonutSlice[]}
            dataKey="value"
            nameKey="name"
            innerRadius="62%"
            outerRadius="92%"
            paddingAngle={data.length > 1 ? 1.5 : 0}
            strokeWidth={0}
            isAnimationActive={animate}
          >
            {data.map((slice, index) => (
              <Cell key={slice.name} fill={slice.color ?? seriesColor(index)} />
            ))}
          </Pie>
        </PieChart>
      </ResponsiveContainer>
      {center ? (
        <div className="pointer-events-none absolute inset-0 flex flex-col items-center justify-center text-center">
          <span className="money text-lg font-semibold">{center.value}</span>
          <span className="text-xs text-muted-foreground">{center.label}</span>
        </div>
      ) : null}
    </Frame>
  );
}

/** A trend in a small space: a balance beside an account's name. */
export function Sparkline({
  values,
  title,
  color = seriesColor(0),
  height = 32,
  className,
}: {
  values: ReadonlyArray<number>;
  title: string;
  color?: string;
  height?: number;
  className?: string;
}) {
  const animate = motionAllowed();
  const data = values.map((value, index) => ({ index, value }));
  return (
    <figure
      role="img"
      aria-label={title}
      data-slot="sparkline"
      className={cn("m-0", className)}
      style={{ height }}
    >
      <ResponsiveContainer width="100%" height="100%">
        <RechartsLineChart data={data} margin={{ top: 2, right: 2, bottom: 2, left: 2 }}>
          <YAxis hide domain={["dataMin", "dataMax"]} />
          <Line
            type="monotone"
            dataKey="value"
            stroke={color}
            strokeWidth={1.5}
            dot={false}
            isAnimationActive={animate}
          />
        </RechartsLineChart>
      </ResponsiveContainer>
    </figure>
  );
}
