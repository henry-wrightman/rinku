#!/usr/bin/env node
/**
 * Hard-gate npm audit for high/critical findings, minus an explicit allowlist.
 * See docs/security/audit-allowlist.md — do not reintroduce continue-on-error.
 */
import { readFileSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const allowlistPath = join(root, "docs/security/npm-audit-allowlist.json");

const allowlist = JSON.parse(readFileSync(allowlistPath, "utf8"));
const allowed = new Set(Object.keys(allowlist.advisories || allowlist));

const result = spawnSync("npm", ["audit", "--json", "--audit-level=high"], {
  cwd: root,
  encoding: "utf8",
  maxBuffer: 20 * 1024 * 1024,
});

let report;
try {
  report = JSON.parse(result.stdout || "{}");
} catch {
  console.error("Failed to parse npm audit JSON");
  console.error(result.stdout?.slice(0, 2000));
  console.error(result.stderr);
  process.exit(1);
}

const blockers = [];
const vulns = report.vulnerabilities || {};
for (const [name, info] of Object.entries(vulns)) {
  if (!["high", "critical"].includes(info.severity)) continue;
  const via = Array.isArray(info.via) ? info.via : [];
  const ghsaIds = via
    .filter((v) => v && typeof v === "object")
    .map((v) => {
      const url = v.url || "";
      const m = url.match(/GHSA-[\w-]+/);
      return m ? m[0] : v.source != null ? String(v.source) : null;
    })
    .filter(Boolean);

  const allAllowed =
    ghsaIds.length > 0 && ghsaIds.every((id) => allowed.has(id));
  // Also allow if the package itself is keyed (legacy)
  if (allAllowed || allowed.has(name)) continue;

  blockers.push({
    name,
    severity: info.severity,
    ghsaIds,
    range: info.range,
  });
}

if (blockers.length === 0) {
  const ignored = [...allowed];
  console.log(
    `npm audit gate passed (high/critical clear; allowlisted: ${ignored.join(", ") || "none"})`,
  );
  process.exit(0);
}

console.error("npm audit hard gate failed — new high/critical issues:");
for (const b of blockers) {
  console.error(
    `  - ${b.severity} ${b.name} ${b.ghsaIds.join(" ") || "(no GHSA)"} range=${b.range}`,
  );
}
console.error(
  "\nFix the dependency, or add a dated entry to docs/security/npm-audit-allowlist.json with reason.",
);
process.exit(1);
