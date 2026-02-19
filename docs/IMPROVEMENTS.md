# ZeroClaw — Ease-of-Use Improvements

Actionable improvement suggestions organized by priority and effort. Each item includes the relevant file path for implementation.

---

## Critical: Core Functionality Gaps

These are features that appear to exist but are incomplete.

### C1. Agent tool calling loop is missing
The agent has tools defined (`shell`, `file_read`, `file_write`, `memory_store`, `memory_recall`, etc.) but `provider.chat_with_system()` is a simple text-in/text-out call — it never invokes tools. Without a tool-use loop (LLM picks tool → execute → inject result → next turn), the agent can't actually *do* things.

**File:** `src/agent/loop_.rs`
**Fix:** Implement a multi-turn tool-calling loop: send tools schema to provider, parse tool-use responses, execute via `SecurityPolicy`, feed results back, repeat until the model produces a final text response.

### C2. Heartbeat only counts tasks, doesn't execute them
`HeartbeatEngine::collect_tasks()` parses `HEARTBEAT.md` and counts lines starting with `- `, but never passes them to the agent for execution.

**File:** `src/heartbeat/engine.rs`
**Fix:** After collecting tasks, invoke the agent for each task (or batch them into a single prompt).

### C3. Cron scheduler has storage but no execution loop
Jobs are stored in SQLite with `due_jobs()` available, but nothing calls it periodically. The daemon doesn't integrate the cron scheduler.

**Files:** `src/cron/mod.rs`, `src/daemon/mod.rs`
**Fix:** Add a scheduler tick in the daemon loop that calls `due_jobs()` every N seconds and dispatches them to the agent.

### C4. Migration from OpenClaw not implemented
`zeroclaw migrate openclaw` is defined in the CLI but the `src/migration/` directory doesn't exist.

**File:** `src/main.rs` (CLI definition), needs `src/migration/mod.rs`
**Fix:** Implement memory import, workspace file migration, and config translation.

### C5. Test infrastructure and CI testing gaps
Tests must compile and pass for development velocity. Pre-existing compilation errors (`AieosIdentity` missing `Default` derive, `Names` test missing `full` field, incorrect fallback test assertion) blocked the entire test suite. There is no integration test that exercises the agent end-to-end (tool calling → execution → result injection), and no CLI smoke test that verifies `zeroclaw onboard --quick` or `zeroclaw status` run without errors.

**Files:** `src/identity.rs`, `tests/`, CI workflow
**Fix (done):** Fixed 3 pre-existing test compilation errors — all 1006+ tests now pass.
**Fix (remaining):**
- Add an end-to-end agent test with a mock LLM provider that returns tool-use responses and verifies the full loop.
- Add CLI smoke tests (`zeroclaw --help`, `zeroclaw status`, `zeroclaw onboard --quick --api-key test`) that verify exit codes.
- Add `cargo test` to CI if not already present.
- Add `cargo clippy -- -D warnings` to CI for lint enforcement.

---

## High Priority

### H1. Onboarding defaults to non-interactive mode
Running `zeroclaw onboard` without flags runs "quick setup" which requires `--api-key`. First-time users should get the interactive wizard automatically.

**File:** `src/main.rs:276-303`
**Fix:** If no config exists and no flags are passed, default to `--interactive`.

### H2. No guidance on how to get API keys
After onboarding without an API key, the output says `export OPENROUTER_API_KEY="sk-..."` but doesn't link to where to get one.

**File:** `src/onboard/wizard.rs`
**Fix:** Add provider-specific URLs: "Get your key at https://openrouter.ai/keys"

### H3. Error messages lack actionable suggestions
Errors like "Failed to create memory backend" or "LLM request failed" don't tell users *why* or *how to fix it*.

**Files:** `src/memory/mod.rs`, `src/providers/mod.rs`, `src/gateway/mod.rs`
**Fix:** Pattern-match on error types and append specific guidance:
- 401/403 → "Check your API key. Run `zeroclaw doctor`."
- Memory init failure → "Check disk space or try `zeroclaw onboard --memory markdown`."
- Channel auth failure → show the rejected username and how to allowlist.

### H4. CLI commands are not discoverable
Commands like `channel doctor`, `integrations info`, `service install`, and `migrate openclaw` are not mentioned in the README help text or grouped by use case.

**File:** `README.md`
**Fix:** Add a "Commands Reference" section grouped by category (Getting Started, Running, Channels, Advanced).

### H5. README leads with benchmarks instead of Quick Start
First-time users want "How do I get started in 2 minutes?" not performance numbers.

**File:** `README.md`
**Fix:** Restructure: What is ZeroClaw → Quick Start (5 commands) → Configuration → Architecture. Move benchmarks to `docs/BENCHMARKS.md`.

### H6. No one-line install
Users must clone and `cargo install --path .` from source.

**Files:** New `install.sh`, Homebrew formula, CI publish workflow
**Fix:** Add `curl -sSfL https://zeroclaw.sh | sh` and/or Homebrew tap. Publish pre-built binaries to GitHub Releases.

### H7. No streaming responses
Both the CLI agent and the web UI wait for the full LLM response before displaying anything. For long responses this feels broken.

**Files:** `src/providers/mod.rs` (trait), `src/agent/loop_.rs`, `ui/index.html`
**Fix:** Add `chat_stream()` to the Provider trait returning a `Stream<Item=String>`. Use SSE for the web UI, line-by-line for CLI.

---

## Medium Priority

### M1. Config validation command
If a user introduces a typo in `config.toml`, the error is unhelpful. Unknown fields are silently ignored.

**File:** `src/config/schema.rs`
**Fix:** Add `zeroclaw config validate` that checks syntax, field types, unknown fields ("did you mean?"), path existence, and enum values.

### M2. Channel testing command
Users configure a channel, start it, and *then* discover it doesn't work.

**File:** `src/channels/mod.rs`
**Fix:** Add `zeroclaw channel test <name>` that validates credentials, attempts connection, and checks allowlist configuration before starting.

### M3. Channel doctor needs richer output
Currently shows "healthy" or "unhealthy" without explaining why or how to fix.

**File:** `src/channels/mod.rs:434-468`
**Fix:** Include error cause, likely explanation, and a fix command. E.g.: "Discord: 401 Unauthorized → Invalid bot token → Run `zeroclaw onboard --channels-only`"

### M4. Environment variable overrides undocumented
ZeroClaw supports `ZEROCLAW_API_KEY`, `ZEROCLAW_PROVIDER`, `PORT`, etc. but this is only visible in source code.

**File:** `src/config/schema.rs:870-927`, `README.md`
**Fix:** Add an "Environment Variables" table to the README listing all supported overrides.

### M5. Memory backend trade-offs not explained
Users don't know when to pick SQLite vs. Markdown vs. None.

**File:** `README.md`
**Fix:** Add a comparison table: SQLite (fast, full search, default) vs. Markdown (git-friendly, basic search) vs. None (stateless).

### M6. No way to inspect stored memories from CLI
Users can only access memory through the agent. No way to browse, search, or debug memory contents.

**File:** New `src/main.rs` subcommand
**Fix:** Add `zeroclaw memory list [--category]` and `zeroclaw memory show <key>`.

### M7. Troubleshooting guide missing
Common issues (API key errors, channel auth, tunnel failures, memory corruption) are scattered or undocumented.

**File:** New `docs/TROUBLESHOOTING.md`
**Fix:** Create a FAQ-style document covering the top 10 error scenarios with step-by-step fixes.

### M8. Skills discovery and validation
Users can't browse available skills or validate that installed skills are well-formed.

**File:** `src/skills/mod.rs`
**Fix:** Add `zeroclaw skills browse` (queries open-skills repo), `zeroclaw skills validate <name>` (checks syntax), and `zeroclaw skills search <query>`.

### M9. Integrations list command missing
Users must know the exact integration name to run `zeroclaw integrations info <name>`. No way to browse.

**File:** `src/integrations/mod.rs`
**Fix:** Add `zeroclaw integrations list [--active|--available|--category <cat>]`. Also make lookup case-insensitive.

### M10. Tunnel failure messages don't explain how to fix
When a tunnel fails, the gateway says "Tunnel failed to start" and falls back silently.

**File:** `src/gateway/mod.rs:260-270`
**Fix:** Add provider-specific fix instructions: Cloudflare → check token URL, Tailscale → install binary, ngrok → get authtoken URL.

### M11. Approval flow for risky commands not implemented
`SecurityPolicy` classifies commands as Low/Medium/High risk, and config has `require_approval_for_medium_risk = true`, but there's no actual approval UI.

**File:** `src/security/mod.rs`
**Fix:** For CLI: prompt user with command preview and [Y/n]. For web UI: modal dialog. For daemon: queue for approval via API.

### M12. No cost tracking or daily limits
`max_cost_per_day_cents` is defined in config but never checked. Token usage is not recorded.

**Files:** `src/providers/mod.rs`, `src/config/schema.rs`
**Fix:** Record token counts from provider responses, estimate cost per model, enforce daily limit, expose via `zeroclaw usage` command.

### M13. Docker image not published
Dockerfile exists but no published image on Docker Hub or GHCR.

**Files:** CI workflow, Dockerfile
**Fix:** Add GitHub Actions workflow to build and push on release. Add `docker pull` instructions to README.

---

## Low Priority

### L1. `zeroclaw status` output is hard to scan
Shows all information in a flat list without visual grouping.

**File:** `src/main.rs:336-402`
**Fix:** Group by section (Configuration, Security, Channels, Runtime) with headers and aligned columns.

### L2. Config template command
No way to see all available config options without reading source code.

**File:** `src/config/schema.rs`
**Fix:** Add `zeroclaw config template` that outputs a fully commented `config.toml` with all defaults and descriptions.

### L3. Pairing code gets lost in terminal scroll
The 6-digit code is printed once at gateway startup and can scroll off screen.

**File:** `src/gateway/mod.rs:285-296`
**Fix:** Add `zeroclaw gateway --show-pairing-code` or write code to a temp file that auto-deletes after first use.

### L4. Cron syntax not documented in help text
Users need to already know cron syntax. No examples in `--help`.

**File:** `src/main.rs` (cron subcommand)
**Fix:** Add examples: `zeroclaw cron add "0 9 * * *" "Daily standup"`, `zeroclaw cron add "*/30 * * * *" "Check emails"`. Add `zeroclaw cron validate` to test expressions.

### L5. Cron job history and logs
Can't see if a cron job ran successfully or what it output.

**File:** `src/cron/mod.rs`
**Fix:** Add execution log table. Add `zeroclaw cron history` and `zeroclaw cron logs <id>`.

### L6. No real-time TUI dashboard
Users running `zeroclaw daemon` have no visibility into system health without checking logs.

**File:** New `src/dashboard/mod.rs`
**Fix:** Add `zeroclaw dashboard` using `ratatui` showing components, recent activity, and health. Alternative: web dashboard at `/dashboard`.

### L7. Structured JSON logging
Logs are unstructured text. Can't integrate with Grafana, Datadog, or CloudWatch.

**File:** `src/main.rs` (tracing setup)
**Fix:** Add `ZEROCLAW_LOG_FORMAT=json` support using `tracing-subscriber`'s JSON layer.

### L8. Prometheus metrics endpoint
Observability config supports `backend = "prometheus"` but it's not wired up.

**File:** `src/observability/mod.rs`, `src/gateway/mod.rs`
**Fix:** Implement Prometheus observer backend. Add `GET /metrics` endpoint to gateway.

### L9. No audit log
Can't see what commands the agent executed, when, or what the outcome was.

**File:** New `src/security/audit.rs`
**Fix:** SQLite table with timestamp, command, risk level, approved/denied, result. Add `zeroclaw security audit` to query.

### L10. Security token rotation
Gateway bearer tokens are permanent once paired. No way to revoke or rotate.

**File:** `src/security/pairing.rs`
**Fix:** Add `zeroclaw security rotate-tokens` and token expiration support.

### L11. Service log viewing
Must manually find log files to see service output.

**File:** `src/service/mod.rs`
**Fix:** Add `zeroclaw service logs [--follow]` that tails the correct log file for the platform.

### L12. Conversation history persistence in web UI
Refreshing the browser loses all chat messages. No IndexedDB or backend storage.

**File:** `ui/index.html`
**Fix:** Store messages in `localStorage` or `IndexedDB`. Optionally expose a `GET /api/conversations` endpoint backed by the memory system.

### L13. Web UI has no config editor
Users must SSH in and edit `config.toml` manually.

**File:** `ui/index.html`, new gateway endpoints
**Fix:** Add `GET /api/config` and `PATCH /api/config` endpoints. Build a form-based editor in the web UI.

### L14. No Windows service support
Service management only supports systemd (Linux) and launchd (macOS).

**File:** `src/service/mod.rs`
**Fix:** Add Windows Task Scheduler or NSSM integration.

---

## Summary Table

| ID | Area | Priority | Effort | Description |
|----|------|----------|--------|-------------|
| C1 | Agent | Critical | High | Implement tool calling loop |
| C2 | Heartbeat | Critical | Medium | Execute tasks, not just count |
| C3 | Cron | Critical | Medium | Wire scheduler into daemon |
| C4 | Migration | Critical | Medium | Implement OpenClaw migration |
| C5 | Testing | Critical | Medium | Test infrastructure + CI gaps |
| H1 | Onboarding | High | Low | Default to interactive on first run |
| H2 | Onboarding | High | Low | Show API key signup URLs |
| H3 | Errors | High | Medium | Add actionable fix suggestions |
| H4 | CLI | High | Low | Document all commands in README |
| H5 | Docs | High | Low | Restructure README for beginners |
| H6 | Install | High | Medium | One-line install + Homebrew |
| H7 | Agent | High | High | Streaming responses |
| M1 | Config | Medium | Medium | `config validate` command |
| M2 | Channels | Medium | Medium | `channel test` command |
| M3 | Channels | Medium | Low | Richer doctor output |
| M4 | Config | Medium | Low | Document env var overrides |
| M5 | Memory | Medium | Low | Backend comparison table |
| M6 | Memory | Medium | Medium | `memory list/show` CLI |
| M7 | Docs | Medium | Low | Troubleshooting guide |
| M8 | Skills | Medium | Medium | Browse, validate, search |
| M9 | Integrations | Medium | Low | `integrations list` command |
| M10 | Tunnel | Medium | Low | Actionable tunnel error messages |
| M11 | Security | Medium | Medium | Approval flow for risky commands |
| M12 | Providers | Medium | Medium | Cost tracking + daily limits |
| M13 | Deploy | Medium | Medium | Publish Docker image |
| L1-L14 | Various | Low | Various | See details above |
