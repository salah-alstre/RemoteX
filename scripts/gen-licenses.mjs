// Generates THIRD-PARTY-LICENSES.txt from Cargo and npm metadata (name, version, SPDX license).
// Run automatically by `npm run build:windows`; the file is bundled into the installer.
import { execSync } from "node:child_process";
import { existsSync, readFileSync, readdirSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const lines = [];

const meta = JSON.parse(execSync("cargo metadata --format-version 1 --locked", { cwd: root, maxBuffer: 1 << 28 }).toString());
const workspace = new Set(meta.workspace_members);
const rust = meta.packages
  .filter((p) => !workspace.has(p.id))
  .map((p) => `${p.name} ${p.version} — ${p.license ?? "see package"}`)
  .sort();

const npmDir = join(root, "node_modules");
const npm = [];
if (existsSync(npmDir)) {
  const scan = (dir, scope = "") => {
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      if (entry.name.startsWith(".")) continue;
      if (entry.name.startsWith("@") && !scope) {
        scan(join(dir, entry.name), entry.name + "/");
        continue;
      }
      const manifest = join(dir, entry.name, "package.json");
      if (existsSync(manifest)) {
        const pkg = JSON.parse(readFileSync(manifest, "utf8"));
        npm.push(`${scope}${entry.name} ${pkg.version} — ${typeof pkg.license === "string" ? pkg.license : (pkg.license?.type ?? "see package")}`);
      }
    }
  };
  scan(npmDir);
}

lines.push("RemoteX third-party components", "==============================", "");
lines.push("RemoteX itself is MIT licensed (see LICENSE). The following components are used under their own licenses.", "");
lines.push(`Rust crates (${rust.length})`, "-".repeat(20), ...rust, "");
lines.push(`JavaScript packages installed for the build (${npm.length})`, "-".repeat(20), ...npm.sort(), "");
writeFileSync(join(root, "THIRD-PARTY-LICENSES.txt"), lines.join("\n"));
console.log(`THIRD-PARTY-LICENSES.txt: ${rust.length} crates, ${npm.length} npm packages`);
