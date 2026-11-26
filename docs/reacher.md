# Local Reacher email verification

Flectar Mail asks [Reacher](https://github.com/reacherhq/check-if-email-exists)
whether an address is deliverable before using it in compose. Reacher is the
open-source backend that checks syntax, MX records and a live SMTP probe. It
runs inside the estate; nothing is sent to a third-party verification service.

## Topology: Reacher runs on `makler`, not on tron

Reacher's useful verdicts need a **live SMTP probe on port 25**. Outbound port
25 is blocked on tron and on home-server, so a Reacher on tron can only ever
answer `unknown` after a timeout. `makler` has port 25 open, so it runs the
probe and tron reaches it over a loopback tunnel.

| Where | What |
| --- | --- |
| **makler** | `/etc/systemd/system/reacher-makler.service` — rootful Podman, image `docker.io/reacherhq/backend:v0.11.11`, `PORT=8080 RCH_HTTP_HOST=0.0.0.0 RCH_ENABLE_BULK=0 RUST_LOG=reacher=info`, published on **127.0.0.1:8084** only, enabled. |
| **tron** | `~/.config/systemd/user/reacher-tunnel.service` — `ssh -N -J server makler@100.123.138.57 -L 127.0.0.1:8085:127.0.0.1:8084` with `SSH_AUTH_SOCK=/run/user/1000/gcr/ssh`, `ExitOnForwardFailure=yes`, `Restart=always`, enabled. |
| **app** | `reacherUrl` in settings is `http://127.0.0.1:8085`. |

Direct SSH to makler from tron times out; the jump host `server` is the path.
The tunnel depends on makler being up **and** the jump host being reachable.

The old local unit `flectar-reacher.service` on tron is **stopped and
disabled** (the file is kept). It cannot succeed without port 25.

## Endpoint

```
POST http://127.0.0.1:8085/v0/check_email
Content-Type: application/json

{"to_email":"contact@victorgirbu.com"}
```

The response contains `is_reachable`: `safe`, `risky`, `invalid` or
`unknown`. `unknown` is normal when the remote server greylists the probe;
with makler's port 25 open the SMTP dialogue usually completes in ~0.3 s.
An SMTP failure is reported separately under `smtp.error`.

Verify it by hand:

```sh
curl -sS -X POST http://127.0.0.1:8085/v0/check_email \
  -H 'Content-Type: application/json' \
  -d '{"to_email":"contact@victorgirbu.com"}'
```

## Start, stop and logs

```sh
# tron: the tunnel
systemctl --user status reacher-tunnel.service
journalctl --user -u reacher-tunnel.service -f

# makler: the probe (passwordless sudo)
ssh -J server makler@100.123.138.57 'sudo systemctl status reacher-makler.service'
```

To update Reacher, change the image tag in makler's unit, then
`systemctl daemon-reload && systemctl restart reacher-makler.service` there.
The tag is pinned deliberately so a restart does not silently change
behaviour.

## App settings and verdict colours

Settings (`crates/flectar-mail-core/src/models.rs`) carry two fields, serialized
in the same camelCase as the rest of `Settings`:

- `reacherUrl` - default `http://127.0.0.1:8084`; this workstation sets
  `http://127.0.0.1:8085` for the tunnel. Empty falls back to the default.
- `reacherEnabled` - default `true`. When off, `Core::reacher_check` returns
  `Unknown` without contacting the service, so writing and sending never depend
  on it.

The client (`crates/flectar-mail-core/src/reacher.rs`) maps the four verdicts
to these colours:

| Verdict | Colour |
| --- | --- |
| `safe` | green |
| `invalid` | red |
| `unknown` | yellow |
| `risky` | purple |

A transport failure or an HTTP 5xx is an error, not a verdict; the caller
should show its message rather than colour an address. Reacher 0.11 has no
`has_risk` field: the client keeps an explicit one if a deployment adds it and
otherwise sets it for `risky`. The SMTP error (`smtp.error`) is preserved as
reason detail for callers that need to explain an `unknown` result.
