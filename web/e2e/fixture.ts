import { spawn } from "node:child_process";
import path from "node:path";
import { createInterface } from "node:readline";

/**
 * The console fixture (crates/console/examples/console_fixture.rs): an
 * embedded Postgres, a store seeded by the simulated controller, and the
 * console serving this app's dist/. Each `advance` drives the controller
 * until request #42 changes column and returns the new column.
 */
export async function startFixture() {
  const binary = process.env.TL3_CONSOLE_FIXTURE;
  if (!binary) {
    throw new Error("Set TL3_CONSOLE_FIXTURE to the built console_fixture example");
  }
  const child = spawn(binary, ["--assets", path.resolve("dist")], {
    stdio: ["pipe", "pipe", "inherit"],
  });
  const lines = createInterface({ input: child.stdout })[Symbol.asyncIterator]();
  // Protocol lines are JSON objects; the embedded Postgres also logs to stdout.
  const next = async (): Promise<Record<string, string>> => {
    for (;;) {
      const line = await lines.next();
      if (line.done) throw new Error("console fixture exited");
      if (line.value.startsWith("{")) return JSON.parse(line.value) as Record<string, string>;
    }
  };
  const ready = await next();
  return {
    url: ready.url!,
    advance: async () => {
      child.stdin.write("advance\n");
      return (await next()).column!;
    },
    stop: () =>
      new Promise<void>((resolve) => {
        child.once("exit", () => resolve());
        child.stdin.end();
      }),
  };
}
