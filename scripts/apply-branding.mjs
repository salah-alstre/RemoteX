// Copies values from branding.json into files that cannot import it at runtime
// (tauri.conf.json, Cargo metadata). Run automatically by the build scripts.
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const brand = JSON.parse(readFileSync(join(root, "branding.json"), "utf8"));
const confPath = join(root, "apps/desktop/src-tauri/tauri.conf.json");
const conf = JSON.parse(readFileSync(confPath, "utf8"));

conf.productName = brand.name;
conf.identifier = brand.identifier;
conf.app.windows[0].title = brand.name;
conf.bundle.publisher = brand.company;
conf.bundle.copyright = `© ${brand.company}`;
// The update endpoint lives with the rest of the infrastructure, not in branding.
const infra = JSON.parse(readFileSync(join(root, "apps/desktop/src-tauri/config/production.json"), "utf8"));
if (infra.updateManifestUrl) conf.plugins.updater.endpoints = [infra.updateManifestUrl];

writeFileSync(confPath, JSON.stringify(conf, null, 2) + "\n");
console.log(`branding applied: ${brand.name} (${brand.identifier})`);
