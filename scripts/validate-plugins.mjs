#!/usr/bin/env node
/**
 * Zero-dependency validator for db-vanguard community driver plugin manifests.
 * Validates all `plugins/*.json` files (excluding `schema.json`) against `plugins/schema.json`.
 */

import fs from "node:fs";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);
const pluginsDir = process.argv[2]
  ? path.resolve(process.argv[2])
  : path.resolve(__dirname, "../plugins");

const schemaPath = path.join(pluginsDir, "schema.json");
if (!fs.existsSync(schemaPath)) {
  console.error(`FAIL: Missing schema file at ${schemaPath}`);
  process.exit(1);
}

const schema = JSON.parse(fs.readFileSync(schemaPath, "utf8"));
const requiredFields = schema.required ?? [];
const properties = schema.properties ?? {};
const allowedKeys = new Set(Object.keys(properties));

function validateManifest(data) {
  const errors = [];

  if (typeof data !== "object" || data === null || Array.isArray(data)) {
    return ["manifest root must be a JSON object"];
  }

  for (const req of requiredFields) {
    if (!(req in data)) {
      errors.push(`missing required field '${req}'`);
    }
  }

  if (schema.additionalProperties === false) {
    for (const key of Object.keys(data)) {
      if (!allowedKeys.has(key)) {
        errors.push(`unexpected field '${key}'`);
      }
    }
  }

  for (const [key, rule] of Object.entries(properties)) {
    if (!(key in data)) continue;
    const value = data[key];
    const allowedTypes = Array.isArray(rule.type) ? rule.type : [rule.type];
    const actualType =
      value === null
        ? "null"
        : Array.isArray(value)
          ? "array"
          : typeof value === "number" && Number.isInteger(value)
            ? "integer"
            : typeof value;

    if (!allowedTypes.includes(actualType)) {
      errors.push(`field '${key}' expected type ${allowedTypes.join("|")}, got ${actualType}`);
      continue;
    }

    if (actualType === "string") {
      if (typeof rule.minLength === "number" && value.length < rule.minLength) {
        errors.push(`field '${key}' length must be >= ${rule.minLength}`);
      }
      if (typeof rule.maxLength === "number" && value.length > rule.maxLength) {
        errors.push(`field '${key}' length must be <= ${rule.maxLength}`);
      }
      if (typeof rule.pattern === "string" && !new RegExp(rule.pattern).test(value)) {
        errors.push(`field '${key}' value '${value}' fails pattern ${rule.pattern}`);
      }
      if (Array.isArray(rule.enum) && !rule.enum.includes(value)) {
        errors.push(`field '${key}' value '${value}' must be one of [${rule.enum.join(", ")}]`);
      }
    }

    if (actualType === "integer") {
      if (typeof rule.minimum === "number" && value < rule.minimum) {
        errors.push(`field '${key}' must be >= ${rule.minimum}`);
      }
      if (typeof rule.maximum === "number" && value > rule.maximum) {
        errors.push(`field '${key}' must be <= ${rule.maximum}`);
      }
    }

    if (actualType === "array") {
      if (typeof rule.minItems === "number" && value.length < rule.minItems) {
        errors.push(`field '${key}' must have at least ${rule.minItems} item(s)`);
      }
      if (rule.uniqueItems && new Set(value).size !== value.length) {
        errors.push(`field '${key}' must contain unique items`);
      }
      if (rule.items && Array.isArray(rule.items.enum)) {
        for (const item of value) {
          if (typeof item !== "string" || !rule.items.enum.includes(item)) {
            errors.push(`field '${key}' contains unsupported item '${String(item)}'`);
          }
        }
      }
    }
  }

  if (
    (data.wire_transport === "tcp" || data.wire_transport === "http") &&
    data.default_port === null
  ) {
    errors.push(`wire_transport '${data.wire_transport}' requires a numeric default_port`);
  }

  return errors;
}

const files = fs
  .readdirSync(pluginsDir)
  .filter((name) => name.endsWith(".json") && name !== "schema.json")
  .sort();

if (files.length === 0) {
  console.error(`FAIL: No plugin manifests found in ${pluginsDir}`);
  process.exit(1);
}

let passCount = 0;
const seenIds = new Set();

for (const file of files) {
  const fullPath = path.join(pluginsDir, file);
  let parsed;
  try {
    parsed = JSON.parse(fs.readFileSync(fullPath, "utf8"));
  } catch (err) {
    console.error(`FAIL  ${file}: invalid JSON (${err.message})`);
    continue;
  }

  const errors = validateManifest(parsed);
  if (typeof parsed.id === "string") {
    if (seenIds.has(parsed.id)) {
      errors.push(`duplicate plugin id '${parsed.id}'`);
    }
    seenIds.add(parsed.id);
  }

  if (errors.length === 0) {
    passCount += 1;
    console.log(`PASS  ${file} (${parsed.id}@${parsed.version})`);
  } else {
    console.error(`FAIL  ${file}: ${errors.join("; ")}`);
  }
}

console.log(`Summary: ${passCount}/${files.length} PASS`);
process.exit(passCount === files.length ? 0 : 1);
