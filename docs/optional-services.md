# Optional services

Yours is a mail client first. Three features use software that is **not part
of the app** and is **not installed by any package**:

| Feature | Service | Default address |
| --- | --- | --- |
| Address checks in the composer | [Reacher](https://github.com/reacherhq/check-if-email-exists) | `http://127.0.0.1:8084` |
| Summary, reply drafts, phishing screen | Any OpenAI-compatible endpoint (Ollama by default) | `http://127.0.0.1:11434/v1` |
| Read aloud and spoken new-mail announcements | Kokoro, Piper, eSpeak NG, Edge TTS, ElevenLabs or a custom speech endpoint | Kokoro `http://127.0.0.1:8766` |

At startup, and whenever **Settings > General > What this app needs > Check
again** is pressed, Yours probes all three with short timeouts. A missing
service does not produce an error dialog or a failed click:

- **Reacher missing** - address checks are skipped entirely. No address chips
  are drawn and no per-recipient request is made. The composer says
  "Address checks unavailable - set a Reacher URL in Settings."
- **AI missing** - the summary button, the composer's AI draft button and the
  phishing switch are disabled, with "AI is not configured" in their hint.
- **No speech engine** - read aloud is disabled, "Announce new mail" stays off,
  and the app never tries to synthesize. Settings > Notifications says why.

## Reacher (address checks)

Reacher runs locally in Docker or Podman. It listens on port 8080 inside the
container; publish it on the app's default 8084:

```sh
docker run --rm -p 127.0.0.1:8084:8080 \
  -e PORT=8080 -e RCH_HTTP_HOST=0.0.0.0 \
  reacherhq/backend:latest
```

Podman is the same command with `podman` in place of `docker`. Keep it
running (add `--restart unless-stopped` and a `docker run -d` if you want a
service). Fedora, Debian/Ubuntu and Arch all package Docker or Podman; the
Reacher image is the same everywhere.

Reacher's useful verdicts need a **live SMTP probe on outbound port 25**. Many
home and cloud networks block that port; Reacher then answers `unknown`, which
Yours shows as "Could not verify" rather than an error. A network with port 25
open gives real "Mailbox exists" answers. See `docs/reacher.md` for a remote
probe setup.

Set a different endpoint under **Settings > General > Address checks**.

## AI (summary, drafts, phishing screen)

The default endpoint is a local [Ollama](https://ollama.com/) on port 11434.
Yours does not install or start Ollama.

### Fedora, Debian/Ubuntu and Arch

```sh
# Fedora and Debian/Ubuntu
curl -fsSL https://ollama.com/install.sh | sh

# Arch
sudo pacman -S ollama

# Then pull a model and make sure the server is running
ollama pull llama3.2
ollama serve        # the installer usually enables the service already
```

Keep the endpoint `http://127.0.0.1:11434/v1` and set the model to a model you
pulled (for example `llama3.2`) under **Settings > AI agents**. The models
field applies the same model to all three tiers.

Any OpenAI-compatible endpoint works instead: LM Studio, llama.cpp's server,
vLLM, or a hosted service. Put its `/v1` base URL and API key under
**Settings > AI agents**. Hosted endpoints must use HTTPS; plain HTTP is
accepted only on loopback.

## Speech (read aloud and announcements)

Install one of these on the machine:

| Engine | Fedora | Debian/Ubuntu | Arch |
| --- | --- | --- | --- |
| eSpeak NG (always works, mechanical voice) | `sudo dnf install espeak-ng` | `sudo apt install espeak-ng` | `sudo pacman -S espeak-ng` |
| Piper (natural on-device voices) | `sudo dnf install piper` | `piper-tts` from your release or [Piper releases](https://github.com/rhasspy/piper/releases) | Piper's release binary or an AUR package |
| Kokoro (best local voice) | run a Kokoro TTS server on `127.0.0.1:8766` | same | same |

Piper also needs at least one `.onnx` voice model in
`~/.local/share/piper-voices` (or the directory set in `ttsVoicesDir`).
Yours picks the first model that is there. eSpeak NG is the fallback when the
chosen engine is missing: Yours never falls silent just because the preferred
voice is not installed.

Edge TTS needs the `edge-tts` command, and ElevenLabs needs a key; both send
the message text online. Choose them under **Settings > General > Read aloud**.
No speech engine means the app stays silent until one is installed.

## Flatpak

**The Flatpak ships only the app.** It does not bundle Reacher, Ollama, any
model weights, or a speech engine. The manifest uses `--share=network`, so
`127.0.0.1` inside the sandbox is the **host's** loopback: a Reacher, Ollama or
Kokoro server running on the host is reachable from the Flatpak without an
override.

```sh
# Host, not inside the sandbox
docker run --rm -p 127.0.0.1:8084:8080 -e PORT=8080 -e RCH_HTTP_HOST=0.0.0.0 reacherhq/backend:latest
ollama serve
```

eSpeak NG is **not bundled** in the Flatpak. Its installed data is about 25 MB,
well over the 5 MB budget, and shipping a subset would still add voice data
the app would have to maintain. The Flatpak therefore reports speech as not
configured unless a host Kokoro server or an online engine is chosen. Install
the host binary for native packages; it is not visible inside the sandbox.

## RPM and DEB

The RPM and DEB packages install the app and its desktop metadata only. They
do not depend on Docker, Reacher, Ollama, model weights or a TTS engine. Use
the commands above on the same machine; the defaults point at loopback, so no
extra configuration is needed once the service is running.

## Checking by hand

```sh
curl -sS http://127.0.0.1:8084/                     # Reacher answers (404 is fine)
curl -sS http://127.0.0.1:11434/v1/models           # Ollama's model list
curl -sS http://127.0.0.1:8766/                     # Kokoro answers
command -v espeak-ng piper edge-tts                 # local speech engines
```

Then press **Check again** in Settings > General. The card shows green
"Connected", grey "Not configured" or red "Unreachable" for each service.
