import {
  Alert,
  AlertDescription,
  Card,
  CardContent,
  CardHeader,
  CardTitle,
  cn,
  Eyebrow,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
} from "@mako-cloud/ui";
import { Info } from "lucide-react";
import { type ReactNode, useEffect, useState } from "react";

import { ApiFailureNotice, type ConsoleApiFailure, toConsoleApiFailure } from "./api-error.js";
import { useManagementClient } from "./management.js";

type ProjectBill = Awaited<ReturnType<ReturnType<typeof useManagementClient>["getProjectBill"]>>;

type TeamBill = Awaited<ReturnType<ReturnType<typeof useManagementClient>["getTeamBill"]>>;

/** A small label over a figure. `dt` keeps the definition list intact, so this
 * is the kit's Eyebrow styling on the term rather than the component. */
const FIGURE_LABEL = "text-xs font-semibold uppercase tracking-wider text-muted-foreground";

/** Micro-dollars as a dollar string. Integer arithmetic end to end; only the
 * display divides. */
function dollars(microDollars: number): string {
  const sign = microDollars < 0 ? "-" : "";
  const absolute = Math.abs(microDollars);
  const whole = Math.floor(absolute / 1_000_000);
  const cents = Math.floor((absolute % 1_000_000) / 10_000);
  return `${sign}$${whole}.${String(cents).padStart(2, "0")}`;
}

function quantity(resource: string, value: number): string {
  if (resource.includes("bytes")) {
    if (value >= 1024 * 1024 * 1024) {
      return `${(value / (1024 * 1024 * 1024)).toFixed(2)} GiB`;
    }
    if (value >= 1024 * 1024) {
      return `${(value / (1024 * 1024)).toFixed(1)} MiB`;
    }
    return `${value} B`;
  }
  return value.toLocaleString("en-US");
}

export function BillingPanel({ teamId }: { readonly teamId: string }) {
  const client = useManagementClient();
  const [bill, setBill] = useState<TeamBill | null>(null);
  const [failure, setFailure] = useState<ConsoleApiFailure | null>(null);
  useEffect(() => {
    let active = true;
    setBill(null);
    setFailure(null);
    void client.getTeamBill(teamId).then(
      (value) => active && setBill(value),
      (error: unknown) => active && setFailure(toConsoleApiFailure(error)),
    );
    return () => {
      active = false;
    };
  }, [client, teamId]);
  const headingId = `billing-${teamId}-title`;

  return (
    <Card className="lg:col-span-2" aria-labelledby={headingId}>
      <CardHeader>
        <CardTitle id={headingId}>Shared usage and plan</CardTitle>
      </CardHeader>
      <CardContent className="grid gap-4">
        <ApiFailureNotice failure={failure} />
        {bill === null && failure === null ? (
          <p className="m-0 text-sm text-muted-foreground">Loading bill…</p>
        ) : bill !== null ? (
          <>
            {/* The notice is the contract of the beta bill: it must be shown,
                not implied, so it renders before any number does. */}
            <Alert role="note">
              <Info aria-hidden="true" />
              <AlertDescription className="block text-foreground">{bill.notice}</AlertDescription>
            </Alert>
            <p className="m-0 text-sm text-muted-foreground">
              Plan: <strong className="text-foreground">{bill.planId}</strong> · period since{" "}
              <time dateTime={bill.periodStart}>{bill.periodStart.slice(0, 10)}</time>
            </p>
            <Table>
              <TableHeader>
                <TableRow className="hover:bg-transparent">
                  <TableHead scope="col">Resource</TableHead>
                  <TableHead scope="col" className="text-right">
                    Used
                  </TableHead>
                  <TableHead scope="col" className="text-right">
                    Included
                  </TableHead>
                  <TableHead scope="col" className="text-right">
                    Overage
                  </TableHead>
                  <TableHead scope="col" className="text-right">
                    Amount
                  </TableHead>
                </TableRow>
              </TableHeader>
              <TableBody>
                {bill.lineItems.map((item) => (
                  <TableRow key={item.resource}>
                    <TableCell className="font-medium">
                      {item.resource.replaceAll("_", " ")}
                    </TableCell>
                    <TableCell className="money">
                      {quantity(item.resource, item.quantity)}
                    </TableCell>
                    <TableCell className="money text-muted-foreground">
                      {quantity(item.resource, item.included)}
                    </TableCell>
                    <TableCell className={cn("money", item.overage > 0 && "text-destructive")}>
                      {quantity(item.resource, item.overage)}
                    </TableCell>
                    <TableCell className="money">{dollars(item.amountMicroDollars)}</TableCell>
                  </TableRow>
                ))}
              </TableBody>
            </Table>
            <dl className="m-0 grid grid-cols-2 gap-3 sm:grid-cols-4">
              <Figure label="Base">{dollars(bill.baseMicroDollars)}</Figure>
              <Figure label="Total">{dollars(bill.totalMicroDollars)}</Figure>
              <Figure label="Credits">{dollars(bill.creditsMicroDollars)}</Figure>
              <Figure label="Balance" negative={bill.balanceMicroDollars < 0}>
                {dollars(bill.balanceMicroDollars)}
              </Figure>
            </dl>
          </>
        ) : null}
      </CardContent>
    </Card>
  );
}

/** One figure of the bill: its name and a number, marked when it has gone negative. */
function Figure({
  label,
  negative,
  children,
}: {
  readonly label: string;
  readonly negative?: boolean;
  readonly children: ReactNode;
}) {
  return (
    <div className="grid content-start gap-0.5 rounded-lg border bg-muted/30 px-3 py-2">
      <dt className={FIGURE_LABEL}>{label}</dt>
      <dd
        className={cn("m-0 text-lg font-semibold tabular-nums", negative && "text-destructive")}
        {...(negative === undefined ? {} : { "data-negative": negative })}
      >
        {children}
      </dd>
    </div>
  );
}

export function ProjectBillingScreen({ projectId }: { readonly projectId: string }) {
  const client = useManagementClient();
  const [bill, setBill] = useState<ProjectBill | null>(null);
  const [failure, setFailure] = useState<ConsoleApiFailure | null>(null);
  useEffect(() => {
    let active = true;
    setBill(null);
    setFailure(null);
    void client.getProjectBill(projectId).then(
      (value) => active && setBill(value),
      (error: unknown) => active && setFailure(toConsoleApiFailure(error)),
    );
    return () => {
      active = false;
    };
  }, [client, projectId]);
  return (
    <section className="grid min-w-0 gap-5" aria-labelledby="project-billing-title">
      <div className="grid gap-1">
        <Eyebrow>This project</Eyebrow>
        <h1 id="project-billing-title" className="text-2xl">
          Billing
        </h1>
        <p className="m-0 text-sm text-muted-foreground">
          Usage costs for this project across all its environments.
        </p>
      </div>
      <ApiFailureNotice failure={failure} />
      {bill === null && failure === null ? (
        <p aria-live="polite" className="m-0 text-sm text-muted-foreground">
          Loading project billing…
        </p>
      ) : null}
      {bill === null ? null : (
        <>
          <Alert role="note">
            <Info aria-hidden="true" />
            <AlertDescription className="block text-foreground">{bill.notice}</AlertDescription>
          </Alert>
          <Card aria-labelledby="project-cost-title">
            <CardHeader>
              <CardTitle id="project-cost-title">Project usage costs</CardTitle>
            </CardHeader>
            <CardContent className="grid min-w-0 gap-4">
              <p className="m-0 text-sm text-muted-foreground">
                Shared plan <strong className="text-foreground">{bill.planId}</strong> ·{" "}
                <time dateTime={bill.periodStart}>{bill.periodStart.slice(0, 10)}</time> to{" "}
                <time dateTime={bill.periodEnd}>{bill.periodEnd.slice(0, 10)}</time>
              </p>
              {new Date(bill.retainedFrom).getTime() > new Date(bill.periodStart).getTime() ? (
                <Alert variant="warning" role="status">
                  <AlertDescription>
                    Usage records begin on {bill.retainedFrom.slice(0, 10)}. Costs cover the
                    retained records only.
                  </AlertDescription>
                </Alert>
              ) : null}
              <dl className="m-0">
                <Figure label="Project usage total">{dollars(bill.totalMicroDollars)}</Figure>
              </dl>
              <Table>
                <TableHeader>
                  <TableRow>
                    <TableHead scope="col">Resource</TableHead>
                    <TableHead scope="col" className="text-right">
                      Project usage
                    </TableHead>
                    <TableHead scope="col" className="text-right">
                      Allocated cost
                    </TableHead>
                  </TableRow>
                </TableHeader>
                <TableBody>
                  {bill.lineItems.map((item) => (
                    <TableRow key={item.resource}>
                      <TableCell className="font-medium">
                        {item.resource.replaceAll("_", " ")}
                      </TableCell>
                      <TableCell className="money">
                        {quantity(item.resource, item.quantity)}
                      </TableCell>
                      <TableCell className="money">{dollars(item.amountMicroDollars)}</TableCell>
                    </TableRow>
                  ))}
                </TableBody>
              </Table>
              <p className="m-0 text-sm text-muted-foreground">
                Each resource's usage charge is shared between projects in proportion to their
                measured usage. The plan's base fee, credits, and balance appear on the overall
                Usage and plan page.
              </p>
              <p className="m-0 text-xs text-muted-foreground">
                Updated{" "}
                <time dateTime={bill.observedAt}>{new Date(bill.observedAt).toLocaleString()}</time>
              </p>
            </CardContent>
          </Card>
        </>
      )}
      <a href="/usage-and-plan" className="text-sm text-primary underline underline-offset-4">
        Overall usage and plan
      </a>
    </section>
  );
}
