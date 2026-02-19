# Guardrails System — Two-Phase Implementation

## Context

ZeroClaw has no guardrail enforcement. LLM responses flow directly to users/channels/gateway with zero filtering. Companies need a way to load custom compliance rules (PII, restricted data, etc.) that are structurally similar to skills but enforce constraints rather than add capabilities. The implementation is split into two phases: prompt-level (advisory) and runtime enforcement (actual filtering).

## Phase 1: Prompt-Level Guardrails (advisory — "the LLM is told not to")

Mirrors the skills system: TOML manifests loaded from directories, rules injected eagerly into the system prompt. No runtime filtering.

### Files to Change

| File | Change |
|------|--------|
| `src/guardrails/mod.rs` | **New** — `Guardrail` struct, `GuardrailRule`, loading from `GUARDRAIL.toml`, `guardrails_to_prompt()` |
| `src/lib.rs` | Add `pub mod guardrails;` |
| `src/channels/mod.rs` | Update `build_system_prompt()` signature to accept `&[Guardrail]`, inject after Safety section |
| `src/agent/loop_.rs` | Call `load_guardrails()`, pass to `build_system_prompt()` |
| `src/config/schema.rs` | Add `GuardrailsConfig` struct + field on `Config` |

### `src/guardrails/mod.rs` (new file)

**Structs:**

```rust
pub struct Guardrail {
    pub name: String,
    pub description: String,
    pub version: String,
    pub author: Option<String>,
    pub severity: Severity,       // "block" or "warn"
    pub rules: Vec<GuardrailRule>,
    pub location: Option<PathBuf>,
}

pub enum Severity {
    Block,  // hard — Phase 2 will enforce at runtime
    Warn,   // advisory — log violation but allow
}

pub struct GuardrailRule {
    pub name: String,
    pub description: String,
    pub kind: RuleKind,
}

pub enum RuleKind {
    /// Instruction injected into system prompt
    Prompt { instruction: String },
    /// Regex pattern for runtime enforcement (Phase 2 — loaded but not enforced yet)
    Regex { pattern: String, action: Action, replacement: Option<String> },
}

pub enum Action { Redact, Block, Warn }
```

**Functions (mirrors `src/skills/mod.rs` patterns):**

- `load_guardrails(workspace_dir: &Path, config: &GuardrailsConfig) -> Vec<Guardrail>` — loads from workspace dir + optional company repo
- `load_guardrails_from_directory(dir: &Path) -> Vec<Guardrail>` — iterate subdirs, parse `GUARDRAIL.toml`
- `load_guardrail_toml(path: &Path) -> Option<Guardrail>` — parse single manifest
- `guardrails_to_prompt(guardrails: &[Guardrail]) -> String` — renders `kind: Prompt` rules eagerly into system prompt text
- `guardrails_dir(workspace_dir: &Path) -> PathBuf` — returns `workspace_dir.join("guardrails")`
- `init_guardrails_dir(workspace_dir: &Path)` — create dir with README

**Prompt injection format** (eager, not on-demand like skills):

```
## Guardrails

The following compliance rules are MANDATORY. Violations will be flagged.

<guardrails>
  <guardrail name="pii-protection" severity="block">
    <rule name="no_ssn_output">Never output Social Security Numbers. Redact as ***-**-****.</rule>
    <rule name="no_credit_cards">Never output credit card numbers. Redact as ****-****-****-****.</rule>
  </guardrail>
</guardrails>
```

Key difference from skills: guardrails are **eagerly injected** (full rule text in system prompt), not lazy/on-demand.

### `src/config/schema.rs` changes

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuardrailsConfig {
    #[serde(default)]
    pub enabled: bool,                    // default: true
    #[serde(default)]
    pub repo: Option<String>,            // company git repo URL (like open-skills)
    #[serde(default)]
    pub extra_dirs: Vec<String>,         // additional guardrail directories
}
```

Add to `Config` struct:
```rust
#[serde(default)]
pub guardrails: GuardrailsConfig,
```

### `src/channels/mod.rs` changes

Update `build_system_prompt()`:
- Add parameter: `guardrails: &[crate::guardrails::Guardrail]`
- Insert guardrails section **between Safety and Skills** (section 2.5) — guardrails override general safety, skills come after
- Call `guardrails::guardrails_to_prompt(guardrails)` and append

### `src/agent/loop_.rs` changes

In the `run()` function (~line 285):
```rust
let guardrails = crate::guardrails::load_guardrails(&config.workspace_dir, &config.guardrails);
// ... pass to build_system_prompt()
```

### GUARDRAIL.toml format

```toml
[guardrail]
name = "pii-protection"
description = "Prevent PII exposure in agent outputs"
version = "1.0.0"
author = "security-team"
severity = "block"

[[rules]]
name = "no_ssn_output"
description = "Never include Social Security Numbers in responses"
kind = "prompt"
instruction = "Never output Social Security Numbers (XXX-XX-XXXX). Redact as ***-**-****."

[[rules]]
name = "no_credit_cards"
description = "Never include credit card numbers"
kind = "prompt"
instruction = "Never output full credit card numbers. Redact as ****-****-****-****."

# Regex rules are loaded but NOT enforced until Phase 2
[[rules]]
name = "ssn_regex_filter"
description = "Runtime SSN redaction"
kind = "regex"
pattern = "\\b\\d{3}-\\d{2}-\\d{4}\\b"
action = "redact"
replacement = "***-**-****"
```

### Tests (in `src/guardrails/mod.rs`)

- TOML parsing: full manifest, missing optional fields, malformed TOML skipped
- Severity enum: block/warn deserialization
- Rule kinds: prompt vs regex
- `guardrails_to_prompt()`: correct XML output, empty input produces empty string
- Directory loading: single guardrail, multiple guardrails, empty dir
- System prompt integration: guardrails appear between Safety and Skills sections

---

## Phase 2: Runtime Input Enforcement (prevents sensitive data from leaving the network)

Adds a `GuardrailEngine` that scans **user input** before it's sent to the LLM provider. The goal is to prevent PII, SSNs, bank account numbers, etc. from ever reaching third-party AI APIs. Inserted at the three input ingestion points — before the provider call, not after.

### Additional Files to Change

| File | Change |
|------|--------|
| `src/guardrails/engine.rs` | **New** — `GuardrailEngine`, compiled regex filters, `scan()` method |
| `src/guardrails/mod.rs` | Add `pub mod engine;`, `build_engine()` function |
| `src/agent/loop_.rs` | Scan user message before `provider.chat_with_system()` (line 150) |
| `src/channels/mod.rs` | Scan inbound channel message before `provider.chat_with_system()` (line 698) |
| `src/gateway/mod.rs` | Scan webhook payload before `state.provider.chat()` (line 500) |

### `src/guardrails/engine.rs` (new file)

```rust
pub struct GuardrailEngine {
    filters: Vec<CompiledFilter>,
}

struct CompiledFilter {
    name: String,
    description: String,
    pattern: regex::Regex,
    action: Action,
    replacement: String,
    severity: Severity,
}

pub struct ScanResult {
    pub text: String,           // filtered text (with redactions applied)
    pub violations: Vec<Violation>,
}

pub struct Violation {
    pub rule: String,           // rule name
    pub matched: String,        // what was matched (redacted in logs)
    pub action: Action,         // what was done
}

impl GuardrailEngine {
    pub fn new(guardrails: &[Guardrail]) -> Self { ... }

    /// Scan user input before it reaches the provider.
    /// - Action::Redact — replace matches in-place, send redacted text to provider
    /// - Action::Block — reject the message entirely, return error to user
    /// - Action::Warn — log warning, send original text (audit trail)
    pub fn scan(&self, input: &str) -> Result<ScanResult> { ... }

    pub fn is_empty(&self) -> bool { ... }
}
```

Uses `regex` crate (already in Cargo.toml via transitive deps — verify, may need explicit dep).

### Insertion Points (all BEFORE the provider call)

**Agent loop** (`src/agent/loop_.rs`):
```rust
// Before line 150 — scan user input before sending to provider
let scan = engine.scan(&conversation)?;
// If action is Block and violations found, return error to user
// If action is Redact, use scan.text (redacted) instead of raw input
let response = provider
    .chat_with_system(Some(system_prompt), &scan.text, model_name, temperature)
    .await?;
```

**Channels** (`src/channels/mod.rs`):
```rust
// Before line 698 — scan inbound channel message
let scan = engine.scan(&msg.content)?;
// Block: reply with "message blocked" to sender, don't call provider
// Redact: send scan.text to provider instead of msg.content
```

**Gateway** (`src/gateway/mod.rs`):
```rust
// Before line 500 — scan webhook payload
let scan = engine.scan(&message)?;
// Block: return 422 with violation details
// Redact: forward scan.text to provider
```

The engine is constructed once at startup and passed as `Arc<GuardrailEngine>` to each codepath.

### GUARDRAIL.toml regex rules (input scanning)

```toml
[[rules]]
name = "ssn_input_filter"
description = "Block SSNs from being sent to AI provider"
kind = "regex"
pattern = "\\b\\d{3}-\\d{2}-\\d{4}\\b"
action = "block"              # reject message entirely
replacement = "***-**-****"   # used if action is "redact"

[[rules]]
name = "credit_card_filter"
description = "Redact credit card numbers before sending to provider"
kind = "regex"
pattern = "\\b\\d{4}[- ]?\\d{4}[- ]?\\d{4}[- ]?\\d{4}\\b"
action = "redact"
replacement = "****-****-****-****"

[[rules]]
name = "bank_account_filter"
description = "Block bank account + routing numbers"
kind = "regex"
pattern = "\\b\\d{9,17}\\b"  # basic; companies would refine
action = "warn"               # log but allow (too broad to block)
```

### Config addition

```toml
[guardrails]
enabled = true
enforce = true              # Phase 2: enable runtime input scanning (default: true)
# repo = "https://github.com/your-company/guardrails.git"
# extra_dirs = ["/etc/zeroclaw/guardrails"]
```

### Tests (in `src/guardrails/engine.rs`)

- Redact action: SSN in input replaced with mask, redacted text sent to provider
- Block action: message rejected with clear error, provider never called
- Warn action: input passed through unchanged, violation logged
- Multiple filters applied in order
- No filters: passthrough (identity)
- Regex compilation failure: skipped with warning, not fatal
- Empty input: no crash
- ScanResult captures all violations for audit logging
- Mixed actions: redact + warn in same input

---

## Verification (both phases)

1. `cargo check` — compiles
2. `cargo test` — all existing tests pass + new guardrails tests
3. Phase 1 manual test: create `~/.zeroclaw/workspace/guardrails/pii/GUARDRAIL.toml`, run agent, verify rules appear in system prompt
4. Phase 2 manual test: add regex rule for SSN pattern, send message containing `123-45-6789`, verify it gets blocked/redacted before reaching the provider
