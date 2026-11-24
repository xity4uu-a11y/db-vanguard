#!/usr/bin/env node
/**
 * 1-click zero-dependency launcher for db-vanguard (`npx create-db-vanguard`).
 * Initializes a local `.db-vanguard.json` workspace profile configuration with
 * ready-to-run Local Demo, SQLite, PostgreSQL, and MySQL connection entries.
 */

import fs from "node:fs";
import path from "node:path";
import process from "node:process";

const VERSION = "1.0.0";

function printHelp() {
  console.log(`create-db-vanguard v${VERSION}

Usage:
  npx create-db-vanguard [options]

Options:
  --dir <path>         Target directory for .db-vanguard.json (default: current directory)
  --engine <name>      Default active connection: local | postgres | sqlite | mysql (default: local)
  --force              Overwrite an existing .db-vanguard.json file
  --dry-run            Print the generated profile configuration without writing to disk
  -v, --version        Display CLI version
  -h, --help           Display this help message
`);
}

function buildWorkspaceConfig(defaultEngine) {
  const validEngines = new Set(["local", "postgres", "sqlite", "mysql"]);
  const activeId = validEngines.has(defaultEngine) ? `${defaultEngine}-default` : "local-default";

  return {
    schemaVersion: "1.0.0",
    activeConnectionId: activeId,
    safetyLinter: {
      blockUnboundedDelete: true,
      blockUnboundedUpdate: true,
      warnOnSelectStar: true,
      warnOnDropOrTruncate: true
    },
    exportDefaults: {
      format: "csv",
      neutralizeCsvFormulas: true,
      sqlInsertBatchSize: 500,
      bufferKiB: 64
    },
    connections: [
      {
        id: "local-default",
        name: "Local E-Commerce Demo (580 rows)",
        driver: "local-engine",
        host: null,
        port: null,
        database: "vanguard_ecommerce_demo",
        username: null,
        readOnly: false
      },
      {
        id: "postgres-default",
        name: "Local PostgreSQL (Wire v3)",
        driver: "postgres-wire-v3",
        host: "127.0.0.1",
        port: 5432,
        database: "postgres",
        username: "postgres",
        readOnly: false
      },
      {
        id: "sqlite-default",
        name: "Local SQLite 3 File",
        driver: "sqlite-wal-native",
        host: null,
        port: null,
        database: "./data/app.sqlite",
        username: null,
        readOnly: false
      },
      {
        id: "mysql-default",
        name: "Local MySQL 8",
        driver: "mysql-protocol-v8",
        host: "127.0.0.1",
        port: 3306,
        database: "app_db",
        username: "root",
        readOnly: false
      }
    ]
  };
}

function main() {
  const args = process.argv.slice(2);
  let targetDir = process.cwd();
  let defaultEngine = "local";
  let force = false;
  let dryRun = false;

  for (let i = 0; i < args.length; i += 1) {
    const arg = args[i];
    if (arg === "-h" || arg === "--help") {
      printHelp();
      return;
    }
    if (arg === "-v" || arg === "--version") {
      console.log(`create-db-vanguard ${VERSION}`);
      return;
    }
    if (arg === "--dry-run") {
      dryRun = true;
      continue;
    }
    if (arg === "--force") {
      force = true;
      continue;
    }
    if (arg === "--dir" && args[i + 1]) {
      targetDir = path.resolve(args[i + 1]);
      i += 1;
      continue;
    }
    if (arg === "--engine" && args[i + 1]) {
      defaultEngine = args[i + 1].toLowerCase();
      i += 1;
      continue;
    }
    console.error(`error: unknown argument '${arg}'. Run with --help for usage.`);
    process.exit(1);
  }

  const config = buildWorkspaceConfig(defaultEngine);
  const serialized = `${JSON.stringify(config, null, 2)}\n`;
  const outputPath = path.join(targetDir, ".db-vanguard.json");

  if (dryRun) {
    console.log(`[dry-run] Target file: ${outputPath}`);
    console.log(serialized);
    return;
  }

  if (fs.existsSync(outputPath) && !force) {
    console.error(`error: ${outputPath} already exists. Pass --force to overwrite.`);
    process.exit(1);
  }

  fs.mkdirSync(targetDir, { recursive: true });
  fs.writeFileSync(outputPath, serialized, "utf8");

  console.log(`Initialized db-vanguard workspace configuration at ${outputPath}`);
  console.log(`Active connection profile : ${config.activeConnectionId}`);
  console.log(`Configured profiles       : ${config.connections.length}`);
  console.log("");
  console.log("Next steps:");
  console.log("  1. Run queries via CLI  : db-vanguard query \"SELECT id, email, plan FROM users LIMIT 10;\"");
  console.log("  2. Stream 50k rows      : db-vanguard bench-export --rows 50000 --format csv");
  console.log("  3. Launch desktop UI    : cd ui && npm install && npm run dev");
}

main();
