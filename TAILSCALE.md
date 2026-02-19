# ZeroClaw + Tailscale (macOS) — Setup & Reference

## Overview

ZeroClaw can expose its gateway to other devices on your Tailscale tailnet using **Tailscale Serve** (tailnet-only) or **Tailscale Funnel** (public internet). There are two ways to use it:

| Mode | How it works | Config |
|------|-------------|--------|
| **Automatic** | ZeroClaw manages `tailscale serve` as a child process | `tunnel.provider = "tailscale"` |
| **Manual** | You run `tailscale serve --bg` yourself; ZeroClaw binds localhost only | `tunnel.provider = "none"` |

---

## Prerequisites

- Tailscale installed and connected (`tailscale status` shows your device)
- MagicDNS enabled in your tailnet admin console
- Client devices have "Use Tailscale DNS settings" enabled

---

## Installing Tailscale on macOS

There are two installation methods. **Pick one and stick with it** — mixing them causes conflicts.

### Option A: Mac App (Recommended)

1. Install from the [Mac App Store](https://apps.apple.com/app/tailscale/id1475387142) or download from [tailscale.com/download](https://tailscale.com/download)
2. Launch the app and sign in

The Mac app installs a CLI wrapper script at `/usr/local/bin/tailscale`:

```bash
# Verify the wrapper exists
cat /usr/local/bin/tailscale
# Should output: #!/bin/sh
#                /Applications/Tailscale.app/Contents/MacOS/tailscale "$@"
```

If the wrapper script is missing (older app versions), create it:

```bash
sudo tee /usr/local/bin/tailscale > /dev/null << 'EOF'
#!/bin/sh
/Applications/Tailscale.app/Contents/MacOS/tailscale "$@"
EOF
sudo chmod +x /usr/local/bin/tailscale
```

Verify:

```bash
tailscale version
tailscale status
```

### Option B: Homebrew CLI

```bash
brew install tailscale
sudo brew services start tailscale
tailscale up
```

This installs its own `tailscaled` daemon and puts `tailscale` in your PATH.

### Do NOT mix both

Using the Mac app's CLI wrapper alongside Homebrew's `tailscaled` causes socket conflicts and errors like:

- `Failed to connect to local Tailscale daemon ... /var/run/tailscaled.socket: no such file`
- `Fatal error: bundleIdentifier is unknown to the registry`

If you hit these, pick one method and uninstall the other.

---

## Automatic Mode (ZeroClaw manages the tunnel)

ZeroClaw spawns `tailscale serve <port>` as a child process and manages its lifecycle.

### Config

```toml
[tunnel]
provider = "tailscale"

[tunnel.tailscale]
funnel = false           # false = tailnet only (serve), true = public internet (funnel)
# hostname = "myhost.tailnet.ts.net"  # optional override; auto-detected if omitted
```

### Usage

```bash
# Start gateway — ZeroClaw starts tailscale serve automatically
zeroclaw gateway

# Or via the daemon
zeroclaw daemon
```

ZeroClaw will:
1. Query `tailscale status --json` to discover your hostname
2. Run `tailscale serve <port>` in the foreground as a managed child process
3. Construct the public URL as `https://<hostname>:<port>`
4. Kill the tunnel on shutdown via `tailscale serve reset`

### When to use automatic mode

- You want ZeroClaw to own the full lifecycle
- You're running via `zeroclaw daemon` or `zeroclaw service`
- Simple single-gateway setup

---

## Manual Mode (you manage the tunnel)

You run `tailscale serve` yourself with full control over HTTPS port, background mode, and options. ZeroClaw just binds to localhost.

### Config

```toml
[tunnel]
provider = "none"        # ZeroClaw doesn't manage the tunnel
```

### Setup

**Terminal 1 — Start the gateway:**

```bash
zeroclaw gateway
# Output: Starting ZeroClaw Gateway on 127.0.0.1:8080
# Output: Pairing code: 386446
```

Verify locally:

```bash
curl -i http://127.0.0.1:8080/health
```

**Terminal 2 — Start Tailscale Serve:**

```bash
# Reset any existing serve config
sudo tailscale serve reset

# Expose localhost:8080 over tailnet HTTPS (background mode)
sudo tailscale serve --bg --https=8080 http://127.0.0.1:8080

# Verify
tailscale serve status
# Expected:
# https://<machine>.<tailnet>.ts.net:8080 (tailnet only)
# |-- / proxy http://127.0.0.1:8080
```

### When to use manual mode

- You need `--bg` (background mode) so the tunnel persists across gateway restarts
- You want custom HTTPS port binding (`--https=8080`)
- You're running multiple services behind Tailscale Serve
- You need `sudo` for the serve command (some setups require it)

---

## Client Access (both modes)

From another device on your tailnet:

### 1. Find your hostname

Check the Tailscale admin console or run on the gateway machine:

```bash
tailscale status --json | python3 -c "import sys,json; print(json.load(sys.stdin)['Self']['DNSName'].rstrip('.'))"
```

Typical format: `<device-name>.<tailnet>.ts.net`

### 2. Verify DNS resolves

```bash
dig +short <machine>.<tailnet>.ts.net
# Should return a 100.x.y.z Tailscale IP
```

### 3. Test health endpoint

```bash
curl -i "https://<machine>.<tailnet>.ts.net:8080/health"
# Expected: HTTP/2 200
```

### 4. Pair (get a bearer token)

Use the one-time pairing code printed by the gateway:

```bash
curl -i -X POST "https://<machine>.<tailnet>.ts.net:8080/pair" \
  -H "X-Pairing-Code: <CODE>"
```

Save the `token` from the JSON response.

### 5. Send messages

```bash
TOKEN="zc_..."
curl -i -X POST "https://<machine>.<tailnet>.ts.net:8080/webhook" \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $TOKEN" \
  -d '{"message":"Hello from remote!"}'
```

---

## Troubleshooting

### "No such file or directory (os error 2)"

ZeroClaw can't find the `tailscale` binary in PATH.

**Fix (Mac app):** Create the wrapper script (see "Installing Tailscale on macOS" above).

**Fix (Homebrew):** Ensure `tailscale` is in PATH: `which tailscale`

**Fix (launchd service):** If running ZeroClaw as a service, PATH may be limited. Add the full path to your service environment or create a symlink.

### "Failed to connect to local Tailscale daemon" / socket errors

You're using a CLI that doesn't match the running daemon.

**Fix:** Use one installation method consistently. If you have the Mac app, use its wrapper. If you use Homebrew, use its daemon.

### "Fatal error: bundleIdentifier is unknown to the registry"

Mixed Mac app + Homebrew CLI installations.

**Fix:** Uninstall one. Either:
- Remove Homebrew: `brew uninstall tailscale && sudo brew services stop tailscale`
- Or remove the Mac app and use Homebrew exclusively

### DNS errors: "Could not resolve host"

MagicDNS isn't working on the client.

**Fix:**
1. Confirm correct hostname from admin console
2. Enable "Use Tailscale DNS settings" on the client
3. Enable MagicDNS in tailnet admin
4. Flush DNS on macOS: `sudo dscacheutil -flushcache && sudo killall -HUP mDNSResponder`
5. Workaround with `--resolve`:
   ```bash
   curl -i --resolve <machine>.<tailnet>.ts.net:8080:<TAILSCALE_IP> \
     "https://<machine>.<tailnet>.ts.net:8080/health"
   ```

### HTTPS by IP fails (TLS alert)

Tailscale Serve uses hostname-based TLS certificates, not IP.

**Fix:** Always use the hostname, not `https://100.x.y.z:PORT`.

### /webhook returns 401 Unauthorized

No token provided, token expired, or gateway restarted (tokens are in-memory).

**Fix:** Re-pair with `POST /pair` and use the new token.

### Curl flags parsed as hostnames

Copy-pasting from docs can introduce Unicode non-breaking spaces.

**Fix:** Re-type the curl command manually, or use a variable:
```bash
TOKEN="zc_..."
curl -i -X POST "https://<machine>.<tailnet>.ts.net:8080/webhook" \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer $TOKEN" \
  -d '{"message":"test"}'
```

---

## Quick Reference

### Gateway machine

```bash
# Automatic mode
zeroclaw gateway                           # tunnel managed by ZeroClaw

# Manual mode
zeroclaw gateway                           # binds localhost only
sudo tailscale serve --bg --https=8080 http://127.0.0.1:8080
tailscale serve status
```

### Client machine

```bash
dig +short <machine>.<tailnet>.ts.net
curl -i "https://<machine>.<tailnet>.ts.net:8080/health"
curl -i -X POST "https://<machine>.<tailnet>.ts.net:8080/pair" -H "X-Pairing-Code: <CODE>"
curl -i -X POST "https://<machine>.<tailnet>.ts.net:8080/webhook" \
  -H "Content-Type: application/json" \
  -H "Authorization: Bearer <TOKEN>" \
  -d '{"message":"hello"}'
```

### Cleanup

```bash
sudo tailscale serve reset    # remove all serve/funnel configs
tailscale serve status        # verify clean
```
