import { expect, type Page, test } from "@playwright/test";

/**
 * Household membership end to end against the in-browser fake, which
 * implements the same five routes as `functions/households/index.ts`: create,
 * invite, accept, role, remove. Every one of them moves a claim, so the app
 * refreshes its session and re-opens what the change affected — which is why
 * each step here waits for the directory to settle before the next.
 *
 * The fake lives in the page, so the whole story runs in one load: reloading
 * would forget the environment along with everything created in it.
 */
const OWNER = "pat@rational.test";
const MEMBER = "sam@rational.test";
const PASSWORD = "RationalDemo1!";

/** The directory has finished its first pull of this generation. */
async function settle(page: Page): Promise<void> {
  await page.waitForFunction(
    () => window.rational.state.directory?.initialSynced === true,
    undefined,
    { timeout: 30_000 },
  );
}

async function enterCredentials(page: Page, email: string, action: string): Promise<void> {
  await page.getByLabel("Email").fill(email);
  await page.getByLabel("Password").fill(PASSWORD);
  await page.getByRole("button", { name: action }).click();
  await page.waitForFunction(() => window.rational.state.phase === "ready");
  await settle(page);
}

async function openHousehold(page: Page): Promise<void> {
  await page.getByRole("link", { name: "Household" }).click();
  await expect(page.getByRole("heading", { name: "Household", exact: true })).toBeVisible();
  await settle(page);
}

test("a household is created, invited to, accepted, re-roled, and removed from", async ({
  page,
}) => {
  await page.goto("/");
  await page.waitForFunction(() => window.rational !== undefined);
  await page.waitForFunction(() => window.rational.state.phase !== "starting");
  await enterCredentials(page, OWNER, "Create account");
  await openHousehold(page);
  await expect(page.getByRole("table", { name: "Members" })).toContainText(OWNER);

  // Creating a household makes this person its owner, which changes their own
  // claims: the session is refreshed and the directory re-opened.
  const before = await page.evaluate(() => window.rational.state.currentHouseholdId);
  const creator = page.getByRole("form", { name: "New household" });
  await creator.getByLabel("Name").fill("Holiday flat");
  await creator.getByLabel("Currency").fill("USD");
  await creator.getByRole("button", { name: "Create household" }).click();
  // Creating opens the new household: the claim is written, the session
  // refreshed, the directory reset, and only then is it selected.
  await page.waitForFunction(
    (previous) =>
      window.rational.state.currentHouseholdId !== previous &&
      window.rational.state.currentHouseholdId !== null,
    before,
    { timeout: 30_000 },
  );
  await settle(page);
  await expect(page.getByRole("combobox", { name: "Household" })).toContainText("Holiday flat");
  await expect(page.getByRole("heading", { name: /Members of Holiday flat/u })).toBeVisible();
  const householdId = await page.evaluate(() => window.rational.state.currentHouseholdId);
  expect(householdId).not.toBeNull();

  // An invitation names an address; the invitee need not have an account yet.
  const invite = page.getByRole("form", { name: "Invite a member" });
  await invite.getByLabel("Email").fill(MEMBER);
  await invite.getByLabel("Role").selectOption("viewer");
  await invite.getByRole("button", { name: "Send invitation" }).click();
  const invited = page.getByTestId(`member-${MEMBER}`);
  await expect(invited).toContainText("invited");
  await expect(invited).toContainText("viewer");

  await page.getByRole("button", { name: "Sign out" }).click();
  await expect(page.getByRole("heading", { name: "Sign in" })).toBeVisible();

  // The invited person signs up and finds the invitation waiting for their
  // address — the only membership document they may read before joining.
  await enterCredentials(page, MEMBER, "Create account");
  await openHousehold(page);
  const invitation = page.getByTestId(`invitation-${householdId}`);
  await expect(invitation).toBeVisible();
  await invitation.getByRole("button", { name: "Accept" }).click();
  await page.waitForFunction(
    (id) => window.rational.state.memberships.some((entry) => entry.household_id === id),
    householdId,
    { timeout: 30_000 },
  );
  await settle(page);
  await expect(page.getByRole("combobox", { name: "Household" })).toContainText(
    "Holiday flat · viewer",
  );

  await page.getByRole("button", { name: "Sign out" }).click();
  await enterCredentials(page, OWNER, "Sign in");
  await openHousehold(page);
  await page.getByRole("combobox", { name: "Household" }).selectOption(householdId as string);
  const memberId = await page.evaluate(
    (address) =>
      window.rational.state.memberships.length >= 0
        ? (window.rational.directory?.session?.collections.memberships
            ?.find()
            .exec()
            .then(
              (documents) =>
                documents
                  .map((document) => document.toJSON())
                  .find(
                    (membership) => membership.email === address && membership.status === "active",
                  )?.user_id ?? null,
            ) ?? null)
        : null,
    MEMBER,
  );
  expect(memberId).not.toBeNull();
  const row = page.getByTestId(`member-${memberId}`);
  await expect(row).toContainText("active");

  // The owner promotes the member, then removes them.
  await row.getByRole("combobox", { name: `Role of ${MEMBER}` }).selectOption("editor");
  await expect(row.getByRole("combobox", { name: `Role of ${MEMBER}` })).toHaveValue("editor");

  await row.getByRole("button", { name: "Remove" }).click();
  await expect(page.getByTestId(`member-${memberId}`)).toHaveCount(0);
});
