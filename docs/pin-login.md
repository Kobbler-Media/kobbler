# PIN-based App Login

Kobbler supports linking external client apps via [configgymajiggy](https://pin.biboop.com) — a temporary PIN-based config exchange service.

## Trust model

The relay is a stateless pairing broker, not a login provider: it only ever carries your
instance's `api_url` and a 90-day admin-scoped `token` between your server and your device, and
only for the lifetime of one PIN (10 minutes, unattended, see below). It is a real dependency,
though — if it's down, pairing a new device is down with it, with no fallback login. The wire
protocol below is fully specified, and the relay itself is
[configgymajiggy](https://github.com/dbrain/configgymajiggy) (`docker compose up -d --build`), so
running your own is a real option: set
`KOBBLER_PIN_SERVICE_URL` on the API, and build each client app that calls the relay directly
against the same URL (kobapp and kobup-android: `-PpinServiceUrl=...`; kobup CLI:
`KOBUP_PIN_SERVICE_URL`).

## Flow

1. **Client app** creates a PIN: `POST https://pin.biboop.com/pin/kobbler` → `{ "pin": "K7M2P9XRJ4" }`
2. **Client app** displays the PIN and starts polling: `POST https://pin.biboop.com/pin/kobbler/K7M2P9XRJ4`
3. **User** opens their Kobbler profile page, enters the PIN, confirms the API URL, and clicks "Link App"
4. **Kobbler** generates a 90-day auth token and pushes config to the PIN
5. **Client app** receives the config on its next poll

PINs expire after 10 minutes of inactivity. Polling counts as activity, so a PIN
stays alive while a client is waiting on it.

Treat the PIN as an opaque string: its length is server-side configuration
(currently 10 characters from `0-9A-Z` minus `I`, `L`, `O` and `U`) and will
change. Do not validate against a fixed length - accept a broad range and let
the service reject what it does not like.

Add `?wait=<seconds>` to the poll to block until the payload arrives instead of
busy-looping.

## Config Payload

When the PIN is fulfilled, the polling response's `result` field contains:

```json
{
  "api_url": "https://kobbler.example.com",
  "token": "aB3xK9m2..."
}
```

| Field     | Type   | Description                                    |
|-----------|--------|------------------------------------------------|
| `api_url` | string | Base URL of the Kobbler instance (no trailing slash) |
| `token`   | string | Bearer token valid for 90 days                 |

## Using the Token

Include the token as a Bearer token in API requests:

```
Authorization: Bearer <token>
```

All API endpoints live under `<api_url>/api/v1/admin/...`. For example:

- `GET <api_url>/api/v1/admin/me` — current user info
- `GET <api_url>/api/v1/admin/books/{id}/audio-files` — list audiobook files
- `GET <api_url>/api/v1/admin/books/{id}/audio/{file_id}` — stream audio (supports HTTP Range)

## Client Implementation Example

```typescript
const PIN_SERVICE = 'https://pin.biboop.com';
const NAMESPACE = 'kobbler';

// 1. Create a PIN
const { pin } = await fetch(`${PIN_SERVICE}/pin/${NAMESPACE}`, { method: 'POST' }).then(r => r.json());

// 2. Display `pin` to user, then poll
// `?wait=30` holds the request open until the payload lands.
async function poll(pin: string): Promise<{ api_url: string; token: string } | null> {
  const resp = await fetch(`${PIN_SERVICE}/pin/${NAMESPACE}/${pin}?wait=30`, { method: 'POST' });

  if (resp.status === 404) return null;      // expired or already consumed
  if (resp.status === 429) return undefined; // throttled - back off and retry
  if (!resp.ok) throw new Error(`PIN service: ${resp.status}`);

  const data = await resp.json();
  return data.result?.api_url ? data.result : undefined; // undefined = still waiting
}
```

## Status codes

| Code | Meaning |
|---|---|
| `200` | PIN allocated, or poll answered |
| `202` | Payload accepted |
| `400` | Malformed namespace or PIN |
| `404` | PIN doesn't exist or expired - allocate a new one and re-prompt |
| `409` | PIN already holds an undelivered payload (double submit) |
| `429` | Too many unknown PINs requested - back off, honour `Retry-After` |
| `503` | Service full or not ready - honour `Retry-After` |

A `404` on poll means the PIN is gone. The service does **not** mint a
replacement - earlier versions did, and clients detected expiry by noticing the
returned `pin` differed from the one requested. That behaviour was removed
because it made PIN probing an allocation primitive and leaked which PINs exist.
Allocate a fresh PIN with `POST /pin/{namespace}` and ask the user to re-enter it.
