# ZeroClaw Web UI — Implementation Handoff

This document describes what was implemented for the ZeroClaw Web UI (Phase 1) and why, for handoff to another LLM or reviewer.

---

## 1. Goal (from the plan)

- Add a **browser UI** so users can:
  1. **Pair** once with the gateway (enter the 6-digit code printed at startup, receive and store a bearer token).
  2. **Chat** with the agent (send messages, see responses).
- The UI is **served by the existing ZeroClaw gateway** (no separate process).
- **No changes** to the REST API contract of `POST /pair` or `POST /webhook`; the frontend only calls these existing endpoints.

---

## 2. What was done

### 2.1 New file: `ui/index.html`

- **Single file:** One HTML document with inline CSS and inline JavaScript. No build step, no Node/npm.
- **Why:** The plan preferred “Option A: Vanilla HTML + CSS + JS” for simplicity and to avoid a build step in the critical path. The UI is embedded in the binary via `include_str!`, so one file keeps embedding trivial.
- **Behavior:**
  - **Storage:** Token is stored in `localStorage` under the key `zeroclaw_bearer`. On load, if a token exists, the chat view is shown; otherwise the pairing view is shown.
  - **Pairing view:** Input for a 6-digit code, “Pair” button. On submit: `fetch("/pair", { method: "POST", headers: { "X-Pairing-Code": code } })`. On 200 and `body.token`, the token is saved and the UI switches to the chat view. On 4xx, `body.error` is shown.
  - **Chat view:** Textarea + “Send” button. On submit: `fetch("/webhook", { method: "POST", headers: { "Authorization": "Bearer " + token, "Content-Type": "application/json" }, body: JSON.stringify({ message }) })`. On 200, the user message and `body.response` are appended to the conversation (with optional `body.model` in the label). On 401, the token is cleared and the pairing view is shown again. On 5xx, `body.error` is shown.
  - **Unpair:** “Unpair and use a different code” clears `zeroclaw_bearer` and shows the pairing view again.
- **URLs:** All requests use relative paths (`/pair`, `/webhook`) so the UI works when served from the same origin as the gateway. No CORS changes were required.

### 2.2 Changes to `src/gateway/mod.rs`

- **Embed the UI:**  
  `const UI_HTML: &str = include_str!("../../ui/index.html");`  
  The path is relative to the file (which is `src/gateway/mod.rs`), so it points at the repo-root `ui/index.html`. The UI is compiled into the binary; no runtime file read.

- **Fallback handler:**  
  New async handler `serve_ui(request: Request<Body>) -> impl IntoResponse`:
  - If `request.method() != Method::GET`, it returns `404 Not Found` (so API routes are not overridden by the fallback).
  - Otherwise it returns `200 OK` with `Content-Type: text/html; charset=utf-8` and the body `UI_HTML`, using Axum’s `Html(UI_HTML)`.

- **Router order:**  
  The router is built with all **API routes first** (e.g. `/health`, `/pair`, `/webhook`, `/whatsapp`), then `.fallback(serve_ui)`. So any request that does not match an API route (e.g. `GET /`, `GET /anything`) is handled by `serve_ui`. Only GET requests get the HTML; POST/GET to unknown paths get 404 from `serve_ui` when not GET.

- **Startup message:**  
  A line was added to the gateway startup print: `GET  / — web UI (pair + chat)` so users know the UI is at the root path.

- **Imports:**  
  Added `Method`, `Request`, `Html`, and `Body` (from `axum::body::Body`) as needed for the fallback handler. No new crates in `Cargo.toml`; only existing axum/tower types are used.

### 2.3 Changes to `README.md`

- **Quick Start:** After the gateway commands, added a sentence that the web UI is at `http://<host>:<port>/` and that users should pair once with the code printed in the terminal, then can chat.
- **Gateway API table:** Added a row for `GET /` describing the web UI (pair once, then chat).
- **Development:** Added a short note that the web UI is embedded (vanilla HTML in `ui/index.html`) and that no separate UI build step is required.

---

## 3. What was explicitly not done (out of scope for Phase 1)

- No WebSocket or OpenClaw protocol.
- No new REST endpoints; only the existing `/pair` and `/webhook` are used.
- No chat history persistence in the UI (only in-session display; backend memory/auto_save is unchanged).
- No config editor, skills UI, cron, or channels UI.
- No CORS changes (UI is same-origin when served from the gateway).

---

## 4. How to verify

1. **Build:**  
   `cargo build --release`  
   Should succeed. The binary includes the contents of `ui/index.html` via `include_str!`.

2. **Run gateway:**  
   `zeroclaw gateway` (or `cargo run --release -- gateway`).  
   On startup, the log should include:  
   `GET  / — web UI (pair + chat)`.

3. **Open UI:**  
   In a browser, open `http://127.0.0.1:8080/` (or the host/port shown).  
   You should see the ZeroClaw page with the pairing form (or the chat form if `zeroclaw_bearer` is already in localStorage).

4. **Pair:**  
   Use the 6-digit code printed in the terminal (e.g. in the “PAIRING REQUIRED” box). Enter it and click “Pair.”  
   The UI should switch to the chat view and the token should be in `localStorage` under `zeroclaw_bearer`.

5. **Chat:**  
   Type a message and click “Send.”  
   The request should be `POST /webhook` with `Authorization: Bearer <token>` and `{"message":"..."}`.  
   The response should be rendered as the assistant reply (and optionally show the model name).

6. **Unpair:**  
   Click “Unpair and use a different code.”  
   The token should be removed and the pairing view shown again.

7. **API routes unchanged:**  
   `GET /health`, `POST /pair`, `POST /webhook` should behave exactly as before (see README “Gateway API” and the plan’s “Relevant API contract”). No changes were made to `handle_health`, `handle_pair`, or `handle_webhook` logic.

---

## 5. File summary

| Path | Change |
|------|--------|
| `ui/index.html` | **New.** Single-page UI: pairing form, chat form, localStorage token, fetch to `/pair` and `/webhook`. |
| `src/gateway/mod.rs` | **Modified.** Added `UI_HTML` constant, `serve_ui` fallback handler, `.fallback(serve_ui)`, GET `/` startup line, and required imports. |
| `README.md` | **Modified.** Quick Start note about web UI, Gateway API row for `GET /`, Development note about embedded UI. |
| `Cargo.toml` | **Unchanged.** No new dependencies. |

---

## 6. Reference

- The implementation follows the **ZeroClaw Web UI Plan** (Phase 1 — Minimal REST UI). The plan is the source of truth for goals, API contract, and out-of-scope items.
- Key plan constraints that were followed: no change to `/pair` or `/webhook` behavior; UI served by the gateway; optional “no build step” choice (vanilla HTML); router order (API routes first, then fallback).
