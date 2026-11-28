# Sender breaches

Yours can mark a sender whose address appears in a known data breach with a
small red skull in the mail list and in the reader header. The skull is backed
by a real lookup, never invented; the popup it opens lists each breach's
title, domain, date and exposed data classes.

## Providers

The default provider is **XposedOrNot, Community Edition**, which needs no API
key and no account:

- The lookup is a GET to
  `https://api.xposedornot.com/v1/breach-analytics?email=<address>`.
  **Only the sender's email address is sent.** No mail content, subject,
  attachment or account credential leaves the machine.
- XposedOrNot keeps no copy of the query; the answer is not persisted upstream
  by Yours, and Yours stores only the verdict in its own `sender_breaches`
  cache. See their [methodology](https://xposedornot.com/methodology).
- Community Edition limits are **2 requests per second per IP** (plus hourly
  and daily caps). Yours paces requests at least 500 ms apart, uses a 6-second
  timeout, backs off on `429` (honouring `Retry-After`) and stops the pass
  instead of retrying in a loop. Mail never waits for a lookup.
- A verdict is cached in `sender_breaches` for **seven days**, including the
  "no breaches" answer, so a busy inbox asks about a sender at most once a
  week.

A **paid Have I Been Pwned key is optional**. Saving a key under **Settings >
General > Sender breach check** switches the whole scan to HIBP's paid
`breachedaccount` endpoint (it is stored in the platform credential store,
never in the mail database or settings blob). With no key, XposedOrNot is used
and the feature works everywhere.

## What the UI says

- **Settings > General** shows a green line while the scan is on:
  *Breach scan is on - XposedOrNot (keyless).* (or *Have I Been Pwned (key
  stored).*). Amber is reserved for a real failure, such as the provider being
  unreachable, and names the reason.
- **What this app needs** lists **Known breaches** with the same state, so the
  feature is never left out of the capability card.
- A hard failure raises one dismissible toast, *Breach scan is on, but the
  breach provider could not be reached - check your connection.* Its button
  opens the settings page. The stored `breachHintShown` flag means it appears
  once, never per message or restart; rate limits are not treated as failures.
- No skull is shown for a sender that was not checked.

## Demo data without the network

To see the skull and its popup offline, start Yours with
`FLECTAR_BREACH_FIXTURE` set to a sender address from the local mailbox:

```sh
FLECTAR_BREACH_FIXTURE=someone@example.com yours
```

At startup Yours stores **one cached demo breach** for that address and reloads
the mail list. The popup labels it *DEMO breach data (no provider lookup was
made)*, and the log says `demo breach data inserted for someone@example.com`.
The variable is inert when unset or when it does not name a valid address, and
this path is compiled out of the Flatpak build. It never performs a lookup and
never claims real breach data.

## Removing demo data

The demo row is an ordinary cached verdict; the next real lookup for that
address replaces it. To remove it by hand, delete the address's row from
`sender_breaches` in the local mail database. (There is no in-app control for
this because the fixture is a developer aid, not a feature.)
