#!/usr/bin/env node
"use strict";
// Launcher: resolve the platform-specific prebuilt binary (installed via
// optionalDependencies, esbuild-style) and exec it with all arguments.
const { spawnSync } = require("child_process");
const path = require("path");

const pkg = `@cialloagent/spacesniff-${process.platform}-${process.arch}`;
const exe = process.platform === "win32" ? "spacesniff.exe" : "spacesniff";

let binary;
try {
  binary = require.resolve(`${pkg}/bin/${exe}`);
} catch {
  console.error(
    `spacesniff: no prebuilt binary for ${process.platform}-${process.arch}.\n` +
      `Your platform may be unsupported, or optional dependencies were not installed\n` +
      `(try reinstalling without --no-optional / --omit=optional).\n` +
      `You can also build from source: cargo install spacesniff`
  );
  process.exit(1);
}

const result = spawnSync(binary, process.argv.slice(2), { stdio: "inherit" });
if (result.error) {
  console.error(`spacesniff: failed to run ${path.basename(binary)}: ${result.error.message}`);
  process.exit(1);
}
process.exit(result.status ?? 1);
