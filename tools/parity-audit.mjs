#!/usr/bin/env node
// Final parity audit (interface request O-11).
//
// Enumerates every file below `packages/**/src` of the TypeScript repo and checks
// it against the parity ledgers of all crates (`crates/*/PARITY.md`): a file is
// covered when a ledger row carries the status `verifiziert`, or when it is named
// in an `Ausschlüsse` table (of a ledger or of the master plan).
//
// The report is written to `plans/final-parity-audit.md`. The tool is a pure
// process helper: it never touches ported code and has no dependencies.
//
// Usage: scripts/parity-audit.sh [--ts-repo <path>] [--out <path>] [--check] [--quiet]

import { execFileSync } from "node:child_process";
import { existsSync, readdirSync, readFileSync, statSync, writeFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const SELF = fileURLToPath(import.meta.url);
const REPO_ROOT = resolve(dirname(SELF), "..");
const MASTER_PLAN = "plans/2026-08-13-rust-port-master-v1.md";

// Status ladder of CONVENTIONS.md §7. `ausgeschlossen` is not part of the ladder;
// it is a separate, equally valid form of coverage.
const STATUS_RANK = { gelesen: 1, portiert: 2, "Tests portiert": 3, verifiziert: 4 };
const SKIP_DIRS = new Set(["node_modules", "dist", ".git", "coverage", ".turbo"]);

function parseArgs(argv) {
  const opts = { tsRepo: resolve(REPO_ROOT, "..", "notagent-main"), out: "plans/final-parity-audit.md", check: false, quiet: false };
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (arg === "--ts-repo") opts.tsRepo = resolve(argv[++i]);
    else if (arg === "--out") opts.out = argv[++i];
    else if (arg === "--check") opts.check = true;
    else if (arg === "--quiet") opts.quiet = true;
    else if (arg === "--explain") opts.explain = argv[++i];
    else if (arg === "--help" || arg === "-h") {
      process.stdout.write(
        "parity-audit — checks every packages/**/src file of the TS repo against the PARITY.md ledgers\n\n" +
          "  --ts-repo <path>  TypeScript repo (default: ../notagent-main)\n" +
          "  --out <path>      report path, repo-relative (default: plans/final-parity-audit.md)\n" +
          "  --check           exit 1 when files without any ledger trace remain\n" +
          "  --quiet           no stdout summary\n" +
          "  --explain <file>  print every ledger trace of one TS file and exit\n",
      );
      process.exit(0);
    } else {
      process.stderr.write(`unknown argument: ${arg}\n`);
      process.exit(2);
    }
  }
  return opts;
}

// ---------------------------------------------------------------- TS repo scan

// Every directory named `src` below `packages/` — that covers the nested
// `packages/session-backends/sqlite-node/src` as well as the flat packages.
function findSrcDirs(tsRepo) {
  const roots = [];
  const walk = (relPath, depth) => {
    if (depth > 4) return;
    for (const entry of readdirSync(join(tsRepo, relPath), { withFileTypes: true })) {
      if (!entry.isDirectory() || SKIP_DIRS.has(entry.name)) continue;
      const child = `${relPath}/${entry.name}`;
      if (entry.name === "src") roots.push(child);
      else walk(child, depth + 1);
    }
  };
  walk("packages", 0);
  return roots.sort();
}

function listFiles(tsRepo, relDir, out) {
  for (const entry of readdirSync(join(tsRepo, relDir), { withFileTypes: true })) {
    if (SKIP_DIRS.has(entry.name)) continue;
    const child = `${relDir}/${entry.name}`;
    if (entry.isDirectory()) listFiles(tsRepo, child, out);
    else if (entry.isFile()) out.push(child);
  }
  return out;
}

function countLines(tsRepo, relPath) {
  try {
    const buf = readFileSync(join(tsRepo, relPath));
    let lines = 0;
    for (const byte of buf) if (byte === 10) lines++;
    return buf.length > 0 && buf[buf.length - 1] !== 10 ? lines + 1 : lines;
  } catch {
    return 0;
  }
}

// --------------------------------------------------------------- markdown bits

function splitRow(line) {
  const trimmed = line.trim().replace(/^\|/, "").replace(/\|$/, "");
  return trimmed.split("|").map((cell) => cell.trim());
}

function isSeparatorRow(line) {
  return /^\|[\s:|-]+\|?\s*$/.test(line.trim()) && line.includes("-");
}

// Brace groups are a common shorthand in the ledgers:
// `src/core/modes/builtin/{auto,manual}/10-*.md` -> one token per alternative.
function expandBraces(text) {
  const match = /\{([^{}]*)\}/.exec(text);
  if (!match) return [text];
  const out = [];
  for (const alternative of match[1].split(",")) {
    out.push(...expandBraces(text.slice(0, match.index) + alternative.trim() + text.slice(match.index + match[0].length)));
  }
  return out;
}

// Pulls path-like tokens out of a table cell: strips markdown decoration, splits on
// punctuation and keeps everything that looks like a path or a file name. Globs (`*`)
// survive, bold markers do not. Line references (`src/models.ts:511-521`) lose their
// suffix, Rust-side paths (`src/keys.rs`) are dropped.
function extractTokens(cell) {
  // Strip bold markers, but keep a `**` that sits inside a path (`src/harness/**`).
  const plain = cell
    .replace(/(^|\s)\*\*|\*\*(\s|$)/g, " ")
    .replace(/`/g, "")
    .replace(/[„“”"]/g, " ");
  const tokens = [];
  for (const braced of expandBraces(plain)) {
    for (const raw of braced.split(/[\s,;()\[\]]+/)) {
      const token = raw
        .replace(/^[.:+/]+/, (m) => (raw.startsWith("./") ? "" : m))
        .replace(/[.,;:]+$/, "")
        .replace(/:\d+(-\d+)?$/, ""); // line reference
      if (!token || token === "—" || token === "-") continue;
      if (!token.includes("/") && !/\.[A-Za-z0-9]+$/.test(token)) continue;
      if (/^https?:/.test(token)) continue;
      if (/\.(rs|toml|lock)$/.test(token)) continue; // Rust side of the port
      tokens.push(token);
    }
  }
  return tokens;
}

// A status cell is read from its beginning: the ledgers write the status first and
// append parentheses or a dash. Substring matching would misread the deviation
// column, where words like "portiert" appear inside prose.
// `ladder: false` marks wordings outside the four values of CONVENTIONS.md §7 —
// they are counted, but reported as a formal finding.
const STATUS_PATTERNS = [
  { re: /^(ausgeschlossen|ausschluss|entfällt|nicht portiert|kein port)\b/, status: "ausgeschlossen", ladder: true },
  { re: /^verifiziert\b/, status: "verifiziert", ladder: true },
  { re: /^tests? portiert\b/, status: "Tests portiert", ladder: true },
  { re: /^portiert\b/, status: "portiert", ladder: true },
  { re: /^gelesen\b/, status: "gelesen", ladder: true },
  { re: /^(vollständig|teilweise|weitgehend)\s+portiert\b/, status: "portiert", ladder: false },
  { re: /^(vollständig|teilweise|weitgehend)\b/, status: "portiert", ladder: false },
  { re: /^task\s+\d+/, status: "portiert", ladder: false },
];

function classifyStatus(text) {
  const value = text.trim().replace(/^\*\*/, "").replace(/`/g, "").toLowerCase();
  for (const pattern of STATUS_PATTERNS) {
    if (pattern.re.test(value)) return { status: pattern.status, ladder: pattern.ladder, text: text.trim() };
  }
  return null;
}

// Walks a markdown document and yields one record per table row:
// { tokens, status, ladder, statusText, heading, line }.
function parseLedgerRows(text) {
  const lines = text.split("\n");
  const rows = [];
  let heading = "";
  let context = ""; // last bold paragraph line — the master plan titles its tables that way
  let table = null;
  let lastLayout = null; // column layout of the last header seen below the current heading
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    if (line.startsWith("#")) {
      heading = line.replace(/^#+\s*/, "").trim();
      context = "";
      table = null;
      lastLayout = null;
      continue;
    }
    if (!line.trim().startsWith("|")) {
      const bold = /^\*\*(.+?)\*\*/.exec(line.trim());
      if (bold) context = bold[1];
      table = null;
      continue;
    }
    if (isSeparatorRow(line)) continue;
    const cells = splitRow(line);
    if (table === null) {
      // A `|` block that starts with a header (separator row underneath) defines the
      // column layout. A block without one continues the previous table — the ledgers
      // separate row groups with blank lines, which ends the table for markdown but
      // not for the audit.
      if (i + 1 < lines.length && isSeparatorRow(lines[i + 1])) {
        const statusIdx = cells.findIndex((cell) => /status/i.test(cell));
        let pathIdx = cells.findIndex((cell) => /(ts-|test)?datei|suite|quelle|stelle|verzeichnis/i.test(cell));
        if (pathIdx < 0) pathIdx = 0;
        const exclusion =
          /ausschl/i.test(heading) || /ausgeschlossen|ausschluss/i.test(context) || cells.some((cell) => /verzeichnis/i.test(cell));
        const readingLog = /lektüre|lektuere/i.test(heading);
        table = { statusIdx, pathIdx, exclusion, readingLog, heading };
        lastLayout = table;
        continue;
      }
      if (lastLayout === null) continue;
      table = { ...lastLayout, heading };
    }
    const tokens = extractTokens(cells[table.pathIdx] ?? "");
    if (tokens.length === 0) continue;
    let verdict = null;
    if (table.exclusion) verdict = { status: "ausgeschlossen", ladder: true, text: "Ausschluss-Tabelle" };
    else if (table.readingLog) verdict = { status: "gelesen", ladder: true, text: "Lektüre-Protokoll" };
    else {
      // Preferred source is the status column; ragged rows (a missing LOC column) push
      // the status one cell to the left, so every cell right of the path is tried.
      verdict = table.statusIdx >= 0 ? classifyStatus(cells[table.statusIdx] ?? "") : null;
      if (!verdict) {
        for (let idx = table.pathIdx + 1; idx < cells.length; idx++) {
          verdict = classifyStatus(cells[idx]);
          if (verdict) break;
        }
      }
    }
    if (!verdict) {
      rows.push({ tokens, status: null, ladder: false, statusText: cells[table.statusIdx] ?? "", heading: table.heading, line: i + 1 });
      continue;
    }
    rows.push({ tokens, status: verdict.status, ladder: verdict.ladder, statusText: verdict.text, heading: table.heading, line: i + 1 });
  }
  return rows;
}

// ------------------------------------------------------------ token resolution

// `*` stays inside one path segment, `**` crosses directories — as in the ledgers
// (`src/api/*.lazy.ts` vs. `src/harness/**`).
function globToRegExp(token) {
  const escaped = token
    .replace(/[.+^${}()|[\]\\]/g, "\\$&")
    .replace(/\*\*/g, " ")
    .replace(/\*/g, "[^/]*")
    .replace(/ /g, ".*");
  return new RegExp(`^${escaped}$`);
}

// Resolves one ledger token against the enumerated src files. Bases are tried in
// order: repo-relative (`packages/...`), package root, package `src/`.
function resolveToken(token, bases, ctx) {
  const isGlob = token.includes("*");
  const isDir = token.endsWith("/");
  const candidates = token.startsWith("packages/") ? [token] : bases.map((base) => (base ? `${base}/${token}` : token));
  for (const candidate of candidates) {
    const cleaned = candidate.replace(/\/+$/, "");
    if (isGlob) {
      const re = globToRegExp(cleaned);
      const files = ctx.srcFiles.filter((file) => re.test(file));
      if (files.length > 0) return { files, kind: "muster", candidate: cleaned };
      continue;
    }
    const abs = join(ctx.tsRepo, cleaned);
    if (!existsSync(abs)) continue;
    if (statSync(abs).isDirectory()) {
      const prefix = `${cleaned}/`;
      return { files: ctx.srcFiles.filter((file) => file.startsWith(prefix)), kind: "verzeichnis", candidate: cleaned };
    }
    if (isDir) continue;
    return { files: ctx.srcFiles.includes(cleaned) ? [cleaned] : [], kind: "datei", candidate: cleaned };
  }
  return null;
}

// A token is worth reporting only when it names a TS-repo file that is really gone:
// globs, bare directory names and paths that exist on the Rust side (`tools/*.mjs`,
// `plans/…`) are not findings.
function isReportableUnresolved(token) {
  if (token.includes("*")) return false;
  if (!/\.[A-Za-z0-9]+$/.test(token) && !token.endsWith("/")) return false;
  if (existsSync(join(REPO_ROOT, token))) return false;
  return /^(packages|src|test|native)\//.test(token);
}

// Some ledgers cite files of a neighbouring package (the sqlite backend implements
// interfaces of `packages/agent`). Such a token is accepted only when exactly one
// package root resolves it — `src/index.ts` exists in every package and stays open.
function resolveTokenAcrossPackages(token, ownBases, ctx) {
  const hits = [];
  for (const base of ctx.allBases) {
    if (ownBases.includes(base)) continue;
    const hit = resolveToken(token, [base], ctx);
    if (hit) hits.push(hit);
  }
  return hits.length === 1 ? hits[0] : null;
}

// ------------------------------------------------------------------- ledger I/O

function readLedgers() {
  const cratesDir = join(REPO_ROOT, "crates");
  const ledgers = [];
  for (const crate of readdirSync(cratesDir).sort()) {
    const relPath = `crates/${crate}/PARITY.md`;
    if (!existsSync(join(REPO_ROOT, relPath))) continue;
    const text = readFileSync(join(REPO_ROOT, relPath), "utf8");
    const source = /TS-Quelle:\s*`([^`]+)`/.exec(text);
    const pkgMatch = source ? /(packages\/[A-Za-z0-9._/-]+)/.exec(source[1]) : null;
    const owner = /—\s*Workstream\s+([ABC])/.exec(text);
    ledgers.push({
      crate,
      path: relPath,
      pkgRoot: pkgMatch ? pkgMatch[1].replace(/\/+$/, "") : null,
      owner: owner ? owner[1] : "?",
      rows: parseLedgerRows(text),
    });
  }
  return ledgers;
}

// Package-level exclusions from the master plan ("Ausgeschlossen" table): only rows
// whose first cell resolves to a real path in the TS repo count.
function readMasterExclusions(ctx) {
  const relPath = MASTER_PLAN;
  const abs = join(REPO_ROOT, relPath);
  if (!existsSync(abs)) return [];
  const text = readFileSync(abs, "utf8");
  const out = [];
  for (const row of parseLedgerRows(text)) {
    if (!/ausgeschlossen|ausschluss/i.test(row.heading) && row.status !== "ausgeschlossen") continue;
    for (const token of row.tokens) {
      const hit = resolveToken(token, [""], ctx);
      if (hit && hit.files.length > 0) out.push({ token, ...hit, source: `${relPath}:${row.line}` });
    }
  }
  return out;
}

// ------------------------------------------------------------------- the audit

function main() {
  const opts = parseArgs(process.argv.slice(2));
  if (!existsSync(opts.tsRepo)) {
    process.stderr.write(`TS repo not found: ${opts.tsRepo}\n`);
    process.exit(2);
  }

  const srcDirs = findSrcDirs(opts.tsRepo);
  const srcFiles = [];
  for (const dir of srcDirs) listFiles(opts.tsRepo, dir, srcFiles);
  srcFiles.sort();
  const ctx = { tsRepo: opts.tsRepo, srcFiles, allBases: [] };

  // package (nearest directory with a package.json above `src`) -> files
  const packages = [...new Set(srcDirs.map((dir) => packageRootOf(opts.tsRepo, dir)))].sort();
  const packageOf = new Map();
  for (const file of srcFiles) {
    const dir = srcDirs.find((srcDir) => file.startsWith(`${srcDir}/`));
    packageOf.set(file, packageRootOf(opts.tsRepo, dir));
  }

  ctx.allBases = packages.flatMap((pkg) => [pkg, `${pkg}/src`]);

  const ledgers = readLedgers();
  const evidence = new Map(); // file -> [{crate, status, heading, line, kind}]
  const unresolved = []; // ledger tokens that point nowhere
  const ladderIssues = []; // rows whose status is not one of the four ladder values
  const owners = new Map(); // package -> owning workstream

  for (const ledger of ledgers) {
    if (ledger.pkgRoot) owners.set(ledger.pkgRoot, ledger.owner);
    const bases = ledger.pkgRoot ? [ledger.pkgRoot, `${ledger.pkgRoot}/src`] : [""];
    for (const row of ledger.rows) {
      let matchedSrc = 0;
      // Cells list sibling files as `src/utils/image-process.ts + image-convert.ts + …`;
      // a bare file name is read relative to the directory of the previous path.
      let siblingDir = null;
      for (const token of row.tokens) {
        // A bare file name is only ever resolved against that sibling directory —
        // against a package root it would silently match the wrong `index.ts`.
        const tokenBases = token.includes("/") ? bases : siblingDir ? [siblingDir] : [];
        const hit =
          resolveToken(token, tokenBases, ctx) ?? (token.includes("/") ? resolveTokenAcrossPackages(token, tokenBases, ctx) : null);
        if (hit && token.includes("/")) siblingDir = hit.candidate.slice(0, hit.candidate.lastIndexOf("/"));
        if (!hit) {
          if (isReportableUnresolved(token)) {
            unresolved.push({ token, ledger: ledger.path, line: row.line, heading: row.heading });
          }
          continue;
        }
        matchedSrc += hit.files.length;
        for (const file of hit.files) {
          if (!evidence.has(file)) evidence.set(file, []);
          evidence.get(file).push({
            crate: ledger.crate,
            status: row.status ?? "unklar",
            statusText: row.statusText,
            heading: row.heading,
            line: row.line,
            kind: hit.kind,
            token,
          });
        }
      }
      // Only rows that actually cover audited files are worth a formal finding.
      if (!row.ladder && matchedSrc > 0) {
        ladderIssues.push({
          ledger: ledger.path,
          line: row.line,
          heading: row.heading,
          tokens: row.tokens,
          status: row.status,
          statusText: (row.statusText ?? "").slice(0, 60),
          files: matchedSrc,
        });
      }
    }
  }

  const masterExclusions = readMasterExclusions(ctx);
  const frameworkExcluded = new Set();
  for (const exclusion of masterExclusions) for (const file of exclusion.files) frameworkExcluded.add(file);

  // Best evidence per file: `verifiziert` wins, then a documented exclusion, then the
  // highest rung of the ladder that was reached.
  const verdicts = new Map();
  for (const file of srcFiles) {
    const traces = evidence.get(file) ?? [];
    let best = null;
    for (const trace of traces) {
      if (best === null) best = trace;
      else if (rank(trace) > rank(best)) best = trace;
    }
    let category;
    if (best && best.status === "verifiziert") category = "verifiziert";
    else if (best && best.status === "ausgeschlossen") category = "ausgeschlossen";
    else if (frameworkExcluded.has(file)) category = "rahmen-ausschluss";
    else if (best) category = "unverifiziert";
    else category = "ohne-nachweis";
    verdicts.set(file, { category, best, traces });
  }

  if (opts.explain) {
    const wanted = opts.explain.replace(/^\.?\//, "");
    const file = srcFiles.find((candidate) => candidate === wanted || candidate.endsWith(`/${wanted}`));
    if (!file) {
      process.stderr.write(`not a file below packages/**/src: ${opts.explain}\n`);
      process.exit(2);
    }
    const { category, traces } = verdicts.get(file);
    process.stdout.write(`${file}\n  Einstufung: ${category}\n`);
    if (traces.length === 0) process.stdout.write("  keine Ledger-Spur\n");
    for (const trace of traces) {
      process.stdout.write(
        `  ${trace.status.padEnd(15)} crates/${trace.crate}/PARITY.md:${trace.line}  [${trace.kind}: ${trace.token}]  — ${trace.heading}\n`,
      );
    }
    return;
  }

  const report = renderReport({
    opts,
    srcDirs,
    srcFiles,
    packages,
    packageOf,
    owners,
    ledgers,
    verdicts,
    unresolved,
    ladderIssues,
    masterExclusions,
  });
  writeFileSync(resolve(REPO_ROOT, opts.out), report);

  const counts = tally(srcFiles, verdicts);
  if (!opts.quiet) {
    process.stdout.write(
      `parity-audit: ${srcFiles.length} Dateien in ${srcDirs.length} src-Verzeichnissen\n` +
        `  verifiziert            ${counts.verifiziert}\n` +
        `  ausgeschlossen         ${counts.ausgeschlossen}\n` +
        `  Rahmen-Ausschluss      ${counts["rahmen-ausschluss"]}\n` +
        `  Ledger < verifiziert   ${counts.unverifiziert}\n` +
        `  ohne Nachweis          ${counts["ohne-nachweis"]}\n` +
        `  unbekannte Ledger-Pfade ${unresolved.length}\n` +
        `Bericht: ${opts.out}\n`,
    );
  }
  if (opts.check && counts["ohne-nachweis"] > 0) process.exit(1);
}

function rank(trace) {
  if (trace.status === "ausgeschlossen") return 3.5; // above `portiert`, below `verifiziert`
  if (trace.status === "unklar") return 0.5; // a row exists, its status is not readable
  return STATUS_RANK[trace.status] ?? 0;
}

// The package a `src` directory belongs to: nearest ancestor with a package.json.
// That keeps `packages/tui/native/*/src` attached to `packages/tui`.
function packageRootOf(tsRepo, srcDir) {
  let dir = srcDir.slice(0, -"/src".length);
  while (dir.includes("/")) {
    if (existsSync(join(tsRepo, dir, "package.json"))) return dir;
    dir = dir.slice(0, dir.lastIndexOf("/"));
  }
  return srcDir.slice(0, -"/src".length);
}

function tally(files, verdicts) {
  const counts = { verifiziert: 0, ausgeschlossen: 0, "rahmen-ausschluss": 0, unverifiziert: 0, "ohne-nachweis": 0 };
  for (const file of files) counts[verdicts.get(file).category]++;
  return counts;
}

// ------------------------------------------------------------------ the report

function renderReport(data) {
  const { opts, srcDirs, srcFiles, packages, packageOf, owners, ledgers, verdicts, unresolved, ladderIssues, masterExclusions } = data;
  const now = new Date(); // local date — the reports are dated in local time
  const today = `${now.getFullYear()}-${String(now.getMonth() + 1).padStart(2, "0")}-${String(now.getDate()).padStart(2, "0")}`;
  let commit = "unbekannt";
  try {
    commit = execFileSync("git", ["-C", REPO_ROOT, "rev-parse", "--short", "HEAD"], { encoding: "utf8" }).trim();
  } catch {
    /* report stays usable without git */
  }

  const byPackage = new Map();
  for (const pkg of packages) byPackage.set(pkg, []);
  for (const file of srcFiles) byPackage.get(packageOf.get(file)).push(file);

  const out = [];
  out.push("# Abschluss-Parity-Audit (Entwurf)");
  out.push("");
  out.push(
    "Maschinell erzeugt von `scripts/parity-audit.sh` (Werkzeug: `tools/parity-audit.mjs`, Auftrag O-11).",
    "Der Bericht prüft jede Datei unter `packages/*/src` des TS-Repos gegen alle `crates/*/PARITY.md`-Ledger.",
    "Er ist ein Entwurf: die Lückenliste ist der Arbeitsvorrat für Gate G4, keine Bewertung.",
    "",
  );
  out.push(`- Lauf: ${today}, Repo-Commit \`${commit}\``);
  out.push(`- TS-Repo: \`${opts.tsRepo}\``);
  out.push(`- Gelesene Ledger: ${ledgers.length} (${ledgers.map((l) => `\`${l.crate}\``).join(", ")})`);
  out.push(`- src-Verzeichnisse: ${srcDirs.length}, Dateien gesamt: ${srcFiles.length}`);
  out.push("");

  const counts = tally(srcFiles, verdicts);
  out.push("## Ergebnis");
  out.push("");
  out.push("| Kategorie | Dateien | Bedeutung |");
  out.push("|---|---:|---|");
  out.push(`| verifiziert | ${counts.verifiziert} | Ledger-Zeile mit Status \`verifiziert\` |`);
  out.push(`| ausgeschlossen | ${counts.ausgeschlossen} | in einer \`Ausschlüsse\`-Tabelle eines Ledgers geführt |`);
  out.push(`| Rahmen-Ausschluss | ${counts["rahmen-ausschluss"]} | im Master-Plan ausgeschlossen, ohne eigenes Ledger |`);
  out.push(`| Ledger < verifiziert | ${counts.unverifiziert} | Ledger-Zeile vorhanden, Status \`gelesen\`/\`portiert\`/\`Tests portiert\` |`);
  out.push(`| ohne Nachweis | ${counts["ohne-nachweis"]} | keine Spur in irgendeinem Ledger — harte Lücke |`);
  out.push("");

  out.push("## Je Paket");
  out.push("");
  out.push("| TS-Paket | Owner | Dateien | verifiziert | ausgeschl. | Rahmen | < verifiziert | ohne Nachweis |");
  out.push("|---|---|---:|---:|---:|---:|---:|---:|");
  for (const [pkg, files] of byPackage) {
    const pkgCounts = tally(files, verdicts);
    out.push(
      `| \`${pkg}\` | ${owners.get(pkg) ?? "—"} | ${files.length} | ${pkgCounts.verifiziert} | ${pkgCounts.ausgeschlossen} | ` +
        `${pkgCounts["rahmen-ausschluss"]} | ${pkgCounts.unverifiziert} | ${pkgCounts["ohne-nachweis"]} |`,
    );
  }
  out.push("");

  out.push("## Lücke 1 — Dateien ohne jede Ledger-Spur");
  out.push("");
  if (counts["ohne-nachweis"] === 0) {
    out.push("Keine. Jede src-Datei ist in mindestens einem Ledger geführt.");
  } else {
    out.push("Diese Dateien tauchen in keinem Ledger auf — weder als Zeile noch als Ausschluss.");
    out.push("");
    for (const [pkg, files] of byPackage) {
      const gaps = files.filter((file) => verdicts.get(file).category === "ohne-nachweis");
      if (gaps.length === 0) continue;
      out.push(`### \`${pkg}\` — ${gaps.length} Dateien (Owner: ${owners.get(pkg) ?? "—"})`);
      out.push("");
      out.push("| Datei | LOC |");
      out.push("|---|---:|");
      for (const file of gaps) out.push(`| \`${file.slice(pkg.length + 1)}\` | ${countLines(opts.tsRepo, file)} |`);
      out.push("");
    }
  }
  out.push("");

  out.push("## Lücke 2 — Ledger-Zeile, aber Status unter `verifiziert`");
  out.push("");
  const soft = srcFiles.filter((file) => verdicts.get(file).category === "unverifiziert");
  if (soft.length === 0) {
    out.push("Keine.");
  } else {
    out.push("Für diese Dateien fehlt der grüne Testnachweis (Status-Leiter aus `CONVENTIONS.md` §7).");
    out.push("");
    out.push("| Datei | bester Status | Beleg | Art |");
    out.push("|---|---|---|---|");
    for (const file of soft) {
      const { best } = verdicts.get(file);
      out.push(`| \`${file}\` | ${best.status} | \`crates/${best.crate}/PARITY.md:${best.line}\` | ${best.kind} |`);
    }
  }
  out.push("");

  out.push("## Lücke 3 — Ledger-Zeilen mit Status außerhalb der Leiter");
  out.push("");
  if (ladderIssues.length === 0) {
    out.push("Keine. Jede Zeile, die eine src-Datei abdeckt, nutzt einen der vier Status-Werte.");
  } else {
    out.push(
      "`CONVENTIONS.md` §7 kennt genau vier Status-Werte (`gelesen`, `portiert`, `Tests portiert`,",
      "`verifiziert`). Diese Zeilen decken src-Dateien ab, schreiben aber etwas anderes in die",
      "Statusspalte; die Prüfung stuft sie höchstens als `portiert` ein. Formalbefund, kein Portmangel.",
      "",
    );
    out.push("| Ledger | Zeile | Statustext | eingestuft als | Dateien |");
    out.push("|---|---:|---|---|---:|");
    for (const issue of ladderIssues) {
      out.push(`| \`${issue.ledger}\` | ${issue.line} | ${issue.statusText || "(leer)"} | ${issue.status ?? "unklar"} | ${issue.files} |`);
    }
  }
  out.push("");

  out.push("## Abdeckung über Sammelzeilen");
  out.push("");
  const collective = new Map();
  for (const file of srcFiles) {
    const { category, best } = verdicts.get(file);
    if (!best || best.kind === "datei") continue;
    if (category !== "verifiziert" && category !== "ausgeschlossen") continue;
    const key = `crates/${best.crate}/PARITY.md:${best.line}|${best.token}|${best.status}`;
    collective.set(key, (collective.get(key) ?? 0) + 1);
  }
  if (collective.size === 0) {
    out.push("Keine. Jede abgedeckte Datei hat eine eigene Ledger-Zeile.");
  } else {
    out.push(
      "Diese Dateien sind nicht einzeln geführt, sondern über eine Verzeichnis- oder Musterzeile.",
      "Das ist zulässig, aber die schwächste Form des Nachweises — an G4 einmal gegenlesen, ob die",
      "Zeile wirklich jede Datei darunter meint (einschränkende Prosa wie „außer X\" wertet die",
      "Prüfung nicht aus).",
      "",
    );
    out.push("| Beleg | Angabe | Status | Dateien |");
    out.push("|---|---|---|---:|");
    for (const [key, count] of [...collective.entries()].sort((a, b) => b[1] - a[1])) {
      const [source, token, status] = key.split("|");
      out.push(`| \`${source}\` | \`${token}\` | ${status} | ${count} |`);
    }
  }
  out.push("");

  out.push("## Rahmen-Ausschlüsse aus dem Master-Plan");
  out.push("");
  if (masterExclusions.length === 0) {
    out.push("Keine auflösbaren Pfadangaben in der Ausschluss-Tabelle des Master-Plans.");
  } else {
    out.push("| Pfad | betroffene src-Dateien | Beleg |");
    out.push("|---|---:|---|");
    for (const exclusion of masterExclusions) {
      out.push(`| \`${exclusion.candidate}\` | ${exclusion.files.length} | \`${exclusion.source}\` |`);
    }
  }
  out.push("");

  out.push("## Unbekannte Ledger-Pfade");
  out.push("");
  if (unresolved.length === 0) {
    out.push("Keine. Jede Pfadangabe in den Ledgern zeigt auf eine existierende Datei oder ein existierendes Verzeichnis.");
  } else {
    out.push("Pfadangaben, die im TS-Repo nicht (mehr) existieren — Tippfehler oder veraltete Zeilen:");
    out.push("");
    out.push("| Ledger | Zeile | Angabe | Abschnitt |");
    out.push("|---|---:|---|---|");
    for (const item of unresolved) out.push(`| \`${item.ledger}\` | ${item.line} | \`${item.token}\` | ${item.heading} |`);
  }
  out.push("");

  out.push("## Methode");
  out.push("");
  out.push(
    "1. **Bestand**: alle Dateien unter jedem `src`-Verzeichnis unterhalb von `packages/` (also auch",
    "   `packages/session-backends/sqlite-node/src`). Keine Filterung nach Endung.",
    "2. **Ledger**: jede Markdown-Tabelle in `crates/*/PARITY.md`. Die Statusspalte liefert den Status",
    "   (`gelesen` → `portiert` → `Tests portiert` → `verifiziert`); Tabellen unter einer",
    "   `Ausschlüsse`-Überschrift zählen als dokumentierter Ausschluss; das Lektüre-Protokoll zählt",
    "   höchstens als `gelesen`.",
    "3. **Pfadauflösung**: Angaben werden relativ zum Paket der Kopfzeile (`TS-Quelle:`), zu dessen `src/`",
    "   oder repo-relativ (`packages/…`) aufgelöst. Verzeichniszeilen (`src/providers/data/`) und Muster",
    "   (`src/api/*.lazy.ts`) gelten für alle darunterliegenden Dateien; solche Belege sind in Lücke 2 als",
    "   `verzeichnis`/`muster` markiert und decken schwächer ab als eine eigene Zeile.",
    "4. **Rangfolge je Datei**: `verifiziert` > dokumentierter Ausschluss > `Tests portiert` > `portiert` >",
    "   `gelesen`. Mehrere Ledger dürfen dieselbe Datei führen (geteilte Pakete wie `coding-agent`).",
    "5. **Nachbarpakete und Geschwisterdateien**: Ein Pfad, den das eigene Paket nicht auflöst, wird gegen",
    "   alle Paketwurzeln probiert und nur bei genau einem Treffer übernommen. Ein blanker Dateiname",
    "   (`… + image-convert.ts + …`) gilt relativ zum Verzeichnis der vorhergehenden Pfadangabe derselben Zeile.",
    "",
    '**Was die Prüfung nicht kann**: Prosa in der Statusspalte („außer `messages.ts`", „Kernfälle") wertet',
    "sie nicht aus, und sie sagt nichts über die inhaltliche Güte eines Ports — nur darüber, ob die",
    "Buchführung eine Aussage zu der Datei enthält.",
    "",
    "Erneut laufen lassen: `scripts/parity-audit.sh` (Optionen: `--ts-repo`, `--out`, `--check`, `--quiet`,",
    "`--explain <datei>`). `--check` endet mit Exit-Code 1, solange Dateien ohne jede Ledger-Spur bleiben —",
    "für Gate G4. `--explain packages/…/foo.ts` zeigt jede Ledger-Spur einer einzelnen Datei.",
  );
  out.push("");
  return out.join("\n");
}

main();
