// Builds the deployable server package:
//   npm run build:server
// Output: dist-server/remotex-server-<version>.zip containing the binary, PowerShell scripts, config template and docs.
import { execSync } from "node:child_process";
import { createHash } from "node:crypto";
import { cpSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const run = (cmd) => execSync(cmd, { cwd: root, stdio: "inherit" });

run("cargo build --release -p remotex-server --locked");

const version = /^version\s*=\s*"([^"]+)"/m.exec(readFileSync(join(root, "Cargo.toml"), "utf8").split("[workspace.package]")[1] ?? "")?.[1] ?? "0.0.0";
const name = `remotex-server-${version}`;
const stage = join(root, "dist-server", name);
rmSync(join(root, "dist-server"), { recursive: true, force: true });
mkdirSync(join(stage, "scripts"), { recursive: true });

cpSync(join(root, "target/release/remotex-server.exe"), join(stage, "remotex-server.exe"));
cpSync(join(root, "server/scripts"), join(stage, "scripts"), { recursive: true });
cpSync(join(root, ".env.example"), join(stage, ".env.example"));
cpSync(join(root, "server/README.md"), join(stage, "README.md"));
cpSync(join(root, "docs/PRIVACY.md"), join(stage, "PRIVACY.md"));

const zip = join(root, "dist-server", `${name}.zip`);
run(`powershell -NoProfile -Command "Compress-Archive -Path '${stage}\\*' -DestinationPath '${zip}' -Force"`);
const sum = createHash("sha256").update(readFileSync(zip)).digest("hex");
writeFileSync(`${zip}.sha256`, `${sum}  ${name}.zip\n`);
console.log(`\nServer package: ${zip}\nSHA-256: ${sum}`);
