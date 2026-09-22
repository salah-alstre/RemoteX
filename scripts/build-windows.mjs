// One command to produce the distributable Windows installers:
//   npm run build:windows
// Output: release/<name>-<version>-setup.exe (NSIS) and release/<name>-<version>.msi (WiX), plus SHA-256 sums.
import { execSync } from "node:child_process";
import { createHash } from "node:crypto";
import { copyFileSync, mkdirSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const run = (cmd, cwd = root) => execSync(cmd, { cwd, stdio: "inherit", env: { ...process.env, REMOTEX_BUILD: process.env.REMOTEX_BUILD ?? new Date().toISOString().slice(0, 10) } });

run("node scripts/apply-branding.mjs");
run("npm ci --workspaces --include-workspace-root");
run("node scripts/gen-licenses.mjs");
// The Windows service is bundled with the app (it is installed by the installers, never by the user).
run("cargo build --release -p remotex-service");
// The overlay adds the service binary, the per-machine install mode and the service install/remove steps.
run("npx tauri build --config src-tauri/tauri.service.conf.json", join(root, "apps/desktop"));

const brand = JSON.parse(readFileSync(join(root, "branding.json"), "utf8"));
const version = JSON.parse(readFileSync(join(root, "apps/desktop/package.json"), "utf8")).version;
const out = join(root, "release");
mkdirSync(out, { recursive: true });

const sums = [];
for (const [dir, ext] of [["nsis", ".exe"], ["msi", ".msi"]]) {
  const folder = join(root, "target/release/bundle", dir);
  for (const file of readdirSync(folder).filter((f) => f.endsWith(ext))) {
    const dest = join(out, file);
    copyFileSync(join(folder, file), dest);
    sums.push(`${createHash("sha256").update(readFileSync(dest)).digest("hex")}  ${file}`);
  }
}
writeFileSync(join(out, "SHA256SUMS.txt"), sums.join("\n") + "\n");
console.log(`\n${brand.name} ${version} installers written to ${out}\n${sums.join("\n")}`);
console.log("\nSign the installers with your code-signing certificate before publishing (see docs/RELEASING.md).");
