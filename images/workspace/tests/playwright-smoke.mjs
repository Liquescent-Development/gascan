import { spawnSync } from "node:child_process";

const chromium = process.env.PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH;
if (!chromium) {
  throw new Error("PLAYWRIGHT_CHROMIUM_EXECUTABLE_PATH is not set");
}

const result = spawnSync(
  chromium,
  [
    "--headless",
    "--no-sandbox",
    "--disable-gpu",
    "--dump-dom",
    "data:text/html,<title>gascan-browser-ok</title>",
  ],
  { encoding: "utf8" },
);
if (result.status !== 0 || !result.stdout.includes("gascan-browser-ok")) {
  process.stderr.write(result.stderr);
  process.exit(1);
}
console.log("browser-ok");
