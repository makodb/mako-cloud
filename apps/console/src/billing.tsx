import {
  Alert,
  AlertDescription,
  Card,
  CardContent,
  CardHeader,
  CardTitle,
  Table,
  TableBody,
  TableCell,
  TableHead,
  TableHeader,
  TableRow,
  cn,
} from "@mako-cloud/ui";
import { Info } from "lucide-react";
import { type ReactNode, useCallback, useEffect, useState } from "react";

import { ApiFailureNotice, type ConsoleApiFailure, toConsoleApiFailure } from "./api-error.js";
import { useManagementClient } from "./management.js";

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
  const reload = useCallback(async () => {
    setFailure(null);
    try {
      setBill(await client.getTeamBill(teamId));
    } catch (error) {
      setFailure(toConsoleApiFailure(error));
    }
  }, [client, teamId]);
  useEffect(() => {
    void reload();
  }, [reload]);

  return (
    <Card className="lg:col-span-2" aria-labelledby="billing-title">
      <CardHeader>
        <CardTitle id="billing-title">Billing</CardTitle>
      </CardHeader>
      <CardContent className="grid gap-4">
        <ApiFailureNotice failure={failure} />
        {bill === null ? (
          <p className="m-0 text-sm text-muted-foreground">Loading bill…</p>
        ) : (
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
        )}
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
