import { defineConfig } from "@playwright/test";

export default defineConfig({
  testDir: "e2e",
  timeout: 90_000,
  workers: 1,
  reporter: [["list"]],
  use: {
    browserName: "chromium",
    viewport: { width: 1600, height: 1000 },
  },
});
