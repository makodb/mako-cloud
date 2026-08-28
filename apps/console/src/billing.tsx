import { useCallback, useEffect, useState } from "react";

import { ApiFailureNotice, type ConsoleApiFailure, toConsoleApiFailure } from "./api-error.js";
import { useManagementClient } from "./management.js";

type TeamBill = Awaited<ReturnType<ReturnType<typeof useManagementClient>["getTeamBill"]>>;

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
    <section className="panel" aria-labelledby="billing-title">
      <h2 id="billing-title">Billing</h2>
      <ApiFailureNotice failure={failure} />
      {bill === null ? (
        <p>Loading bill…</p>
      ) : (
        <>
          {/* The notice is the contract of the beta bill: it must be shown,
              not implied, so it renders before any number does. */}
          <p role="note">{bill.notice}</p>
          <p>
            Plan: <strong>{bill.planId}</strong> · period since{" "}
            <time dateTime={bill.periodStart}>{bill.periodStart.slice(0, 10)}</time>
          </p>
          <div className="table-scroll">
            <table>
              <thead>
                <tr>
                  <th scope="col">Resource</th>
                  <th scope="col">Used</th>
                  <th scope="col">Included</th>
                  <th scope="col">Overage</th>
                  <th scope="col">Amount</th>
                </tr>
              </thead>
              <tbody>
                {bill.lineItems.map((item) => (
                  <tr key={item.resource}>
                    <td>{item.resource.replaceAll("_", " ")}</td>
                    <td>{quantity(item.resource, item.quantity)}</td>
                    <td>{quantity(item.resource, item.included)}</td>
                    <td>{quantity(item.resource, item.overage)}</td>
                    <td>{dollars(item.amountMicroDollars)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          <dl>
            <div>
              <dt>Base</dt>
              <dd>{dollars(bill.baseMicroDollars)}</dd>
            </div>
            <div>
              <dt>Total</dt>
              <dd>{dollars(bill.totalMicroDollars)}</dd>
            </div>
            <div>
              <dt>Credits</dt>
              <dd>{dollars(bill.creditsMicroDollars)}</dd>
            </div>
            <div>
              <dt>Balance</dt>
              <dd data-negative={bill.balanceMicroDollars < 0}>
                {dollars(bill.balanceMicroDollars)}
              </dd>
            </div>
          </dl>
        </>
      )}
    </section>
  );
}
