# Local Reacher email verification

Flectar Mail asks [Reacher](https://github.com/reacherhq/check-if-email-exists)
whether an address is deliverable before using it in compose. Reacher is the
open-source backend that checks syntax, MX records and a live SMTP probe. It
runs on this workstation only; nothing is sent to a third-party verification
service.

## What is installed

- A rootless **Podman** container named `flectar-reacher`, from the pinned
  image `docker.io/reacherhq/backend:v0.11.11`.
- A systemd **user** unit named `flectar-reacher.service` at
  `~/.config/systemd/user/flectar-reacher.service`. It is enabled in
  `default.target`; user lingering is on for `tron`, so it starts on boot
  and survives logout.
- Only `127.0.0.1:8084` is published (host loopback). The container listens
  on 8080 internally. Do not change the publish address to `0.0.0.0`: the
  service makes outbound SMTP probes and must stay local.

## Endpoint

```
POST http://127.0.0.1:8084/v0/check_email
Content-Type: application/json

{"to_email":"contact@victorgirbu.com"}
```

The response contains `is_reachable`: `safe`, `risky`, `invalid` or
`unknown`. `unknown` is normal when the remote server greylists the probe or
when outbound port 25 is blocked. An SMTP failure is reported separately under
`smtp.error`.

Verify it by hand:

```sh
curl -sS -X POST http://127.0.0.1:8084/v0/check_email \
  -H 'Content-Type: application/json' \
  -d '{"to_email":"contact@victorgirbu.com"}'
```

## Start, stop and logs

```sh
systemctl --user status flectar-reacher.service
systemctl --user start flectar-reacher.service
systemctl --user stop flectar-reacher.service
systemctl --user restart flectar-reacher.service
journalctl --user -u flectar-reacher.service -f
```

The unit is enabled, so no manual start is needed after a reboot. To update
Reacher, change the image tag in the unit, then run `systemctl --user
daemon-reload && systemctl --user restart flectar-reacher.service`. The tag is
pinned deliberately so a restart does not silently change behaviour.

## App settings and verdict colours

Settings (`crates/flectar-mail-core/src/models.rs`) carry two fields, serialized
in the same camelCase as the rest of `Settings`:

- `reacherUrl` - default `http://127.0.0.1:8084`; empty falls back to it.
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
