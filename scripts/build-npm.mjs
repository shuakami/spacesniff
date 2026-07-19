#!/usr/bin/env node
// Generates the per-platform npm packages from prebuilt binaries.
//
// Usage: node scripts/build-npm.mjs <binaries-dir>
//   <binaries-dir> must contain one subdirectory per npm platform id
//   (win32-x64, linux-x64, darwin-arm64, ...), each with the compiled
//   spacesniff[.exe] binary inside (as produced by the release workflow).
//
// Output: npm/spacesniff-<platform>-<arch>/ packages ready for `npm publish`.
import fs from "node:fs";
import path from "node:path";

const root = path.join(import.meta.dirname, "..");
const binDir = process.argv[2];
if (!binDir) {
  console.error("usage: node scripts/build-npm.mjs <binaries-dir>");
  process.exit(1);
}

const mainPkg = JSON.parse(
  fs.readFileSync(path.join(root, "npm/spacesniff/package.json"), "utf8")
);
const version = mainPkg.version;

for (const platformId of fs.readdirSync(binDir)) {
  const [os, cpu] = platformId.split("-");
  const exe = os === "win32" ? "spacesniff.exe" : "spacesniff";
  const src = path.join(binDir, platformId, exe);
  if (!fs.existsSync(src)) {
    console.error(`skip ${platformId}: ${src} not found`);
    continue;
  }
  const pkgDir = path.join(root, `npm/spacesniff-${platformId}`);
  fs.mkdirSync(path.join(pkgDir, "bin"), { recursive: true });
  fs.copyFileSync(src, path.join(pkgDir, "bin", exe));
  if (os !== "win32") fs.chmodSync(path.join(pkgDir, "bin", exe), 0o755);
  fs.writeFileSync(
    path.join(pkgDir, "package.json"),
    JSON.stringify(
      {
        name: `spacesniff-${platformId}`,
        version,
        description: `spacesniff prebuilt binary for ${platformId}`,
        license: "MIT",
        repository: mainPkg.repository,
        os: [os],
        cpu: [cpu],
      },
      null,
      2
    ) + "\n"
  );
  console.log(`generated npm/spacesniff-${platformId}`);
}
