# Contributing to db-vanguard

We welcome contributions to `vanguard-core`, the Tauri v2 desktop shell, the React 19 SQL Studio UI, and especially community database drivers.

## Design & Engineering Standards

- **Zero C link dependencies in `vanguard-core`**: Core drivers and parsers must compile cleanly in pure Rust without requiring system `libssl`, `libsqlite3`, or C toolchains.
- **Constant-memory streaming**: Exporters and wire-protocol drivers must stream rows incrementally rather than buffering full result sets into memory.
- **Apple-inspired minimalist UI**: Use restrained neutral palettes, 1px hairline borders, crisp monospace typography, and muted semantic Pass/Warn/Fail states. Avoid emojis, neon gradients, and visual clutter across code, UI, logs, and documentation.
- **Zero warnings policy**: All Rust changes must pass `cargo clippy -p vanguard-core --all-targets -- -D warnings` and all TypeScript changes must pass `tsc --noEmit`.

## Adding a Community Database Driver

1. **Create a manifest in `plugins/<driver-id>.json`** conforming to `plugins/schema.json`:
   - `id`: lowercase kebab-case identifier (e.g., `cockroachdb-wire-v3`)
   - `wire_transport`: `tcp`, `file`, `http`, or `in_process`
   - `default_port`: numeric port for `tcp`/`http` transports, or `null` for `file`/`in_process`
   - `capabilities`: one or more of `query`, `schema_inspect`, `streaming_export`, `transactions`, `explain`, `wal_inspect`, `parquet_scan`, `columnar_batch`
   - `streaming_export` & `zero_c_deps`: truthful boolean flags
2. **Validate your manifest** using both validators:
   ```bash
   node scripts/validate-plugins.mjs
   cargo run -p vanguard-core -- validate-plugins --dir plugins
   ```
3. **Implement `DatabaseDriver` in Rust** (`crates/vanguard-core/src/drivers/`):
   - Implement `connect`, `is_connected`, `list_tables`, `describe_table`, `execute_query`, and `stream_export`.
   - Include deterministic in-process protocol/mock tests so CI verifies wire framing without external database servers.

## Local Development Workflow

```bash
# 1. Run Rust unit tests & strict clippy
cargo test -p vanguard-core -- --nocapture
cargo clippy -p vanguard-core --all-targets -- -D warnings

# 2. Validate community driver manifests
node scripts/validate-plugins.mjs

# 3. Verify UI typecheck, Vitest suite, and Vite production build
cd ui
npm install
npm run typecheck
npm test
npm run build
```

## Pull Request Checklist

- All existing and new unit tests pass (`cargo test` and `npm test`).
- Zero clippy warnings and zero TypeScript errors.
- Community plugin manifests pass `node scripts/validate-plugins.mjs`.
- No secrets, local paths, or temporary database files are staged.
