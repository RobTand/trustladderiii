import { expect, test } from "@playwright/test";
import { startFixture } from "./fixture";

const LABELS: Record<string, string> = { NeedsRob: "Needs Rob" };

test("a controller commit moves a card on the open board", async ({ page }) => {
  const fixture = await startFixture();
  try {
    await page.goto(fixture.url);
    const card = (region: string) =>
      page.getByRole("region", { name: region }).getByRole("article", { name: /tl3-sandbox#42:/ });
    await expect(card("Intake")).toBeVisible();
    // The example config's request reader is default-model; cards name it briefly.
    await expect(card("Intake").getByText("Reader · default-model", { exact: true })).toBeVisible();
    await expect(page.getByRole("status")).toHaveText(/Live/);
    // A reload would drop this marker.
    await page.evaluate(() => ((window as unknown as { tl3Marker: number }).tl3Marker = 1));
    const column = await fixture.advance();
    expect(column).not.toBe("Intake");
    await expect(card(LABELS[column] ?? column)).toBeVisible();
    await expect(card("Intake")).toHaveCount(0);
    expect(await page.evaluate(() => (window as unknown as { tl3Marker?: number }).tl3Marker)).toBe(1);
  } finally {
    await fixture.stop();
  }
});

test("board screenshots in both themes", async ({ page }) => {
  test.skip(!process.env.TL3_SCREENSHOTS, "set TL3_SCREENSHOTS to a directory to record them");
  const fixture = await startFixture();
  try {
    // Wide enough to show every column.
    await page.setViewportSize({ width: 2720, height: 1100 });
    for (const scheme of ["light", "dark"] as const) {
      await page.emulateMedia({ colorScheme: scheme });
      await page.goto(fixture.url);
      await expect(page.getByRole("status")).toHaveText(/Live/);
      await page.screenshot({ path: `${process.env.TL3_SCREENSHOTS}/board-${scheme}.png` });
    }
    await page.setViewportSize({ width: 390, height: 844 });
    await page.screenshot({ path: `${process.env.TL3_SCREENSHOTS}/board-phone.png` });
  } finally {
    await fixture.stop();
  }
});
