# Deploying Kobbler Behind a Reverse Proxy

Guide for deploying Kobbler with Nginx Proxy Manager, Traefik, Caddy, or similar reverse proxies.

## Architecture Overview

Kobbler exposes three HTTP endpoints:

| Endpoint | Default Port | Purpose |
|----------|--------------|---------|
| Web UI | 5173 | SvelteKit web UI (nginx proxies /api internally) |
| API | 6766 | Direct REST API access (for Calibre plugin) |
| Kobo Sync | 6767 | Kobo device sync proxy |

The web container's nginx already proxies `/api/*` to the API container, so you only need to configure routing for:
- Web UI (everything except /kobo)
- Kobo sync (/kobo paths)

## URL Configuration

Set the public URL in `config/kobbler.toml`:

```toml
# For path-based routing (recommended)
public_proxy_url = "https://kobbler.example.com/kobo"
```

This URL is used to generate download links that Kobo devices use. It must match your external URL exactly.

## Deployment Options

### Option 1: Single Domain with Path Routing (Recommended)

Route all traffic through one domain using path-based routing:

- Web UI: `https://kobbler.example.com/`
- API: `https://kobbler.example.com/api/*` (proxied by web container)
- Kobo Sync: `https://kobbler.example.com/kobo/*`

```toml
# config/kobbler.toml
public_proxy_url = "https://kobbler.example.com/kobo"
```

Device sync URL: `https://kobbler.example.com/kobo/{token}`

### Option 2: Separate Subdomain

Use a dedicated subdomain for Kobo sync:

- Web/API: `https://kobbler.example.com`
- Kobo Sync: `https://kobo.example.com`

```toml
# config/kobbler.toml
public_proxy_url = "https://kobo.example.com"
```

Device sync URL: `https://kobo.example.com/{token}`

### Option 3: Separate Port (Direct Access)

For local network or simple setups without a reverse proxy:

```toml
# config/kobbler.toml
public_proxy_url = "http://192.0.2.20:6767"
```

Device sync URL: `http://192.0.2.20:6767/{token}`

## Nginx Proxy Manager Setup

### Single Domain Configuration

**1. Create Proxy Host**

Details Tab:
- Domain Names: `kobbler.example.com`
- Scheme: `http`
- Forward Hostname/IP: `web` (or your web container name/IP)
- Forward Port: `80`
- Websockets Support: ON

**2. SSL Tab**

- Request a new SSL Certificate
- Force SSL: ON
- HTTP/2 Support: ON

**3. Custom Locations Tab**

Add a custom location for the Kobo sync endpoint:

- Location: `/kobo/`
- Scheme: `http`
- Forward Hostname/IP: Your API container IP (e.g., `192.0.2.20`) - **no trailing slash**
- Forward Port: `6767`

**Important:** Do not add a trailing slash to the hostname - this can break proxying.

**4. Advanced Tab — Proxy Buffers**

The Kobo sync protocol uses large HTTP headers (sync tokens can exceed 4KB). Add this to the **Advanced** tab of the proxy host to prevent 502 errors:

```nginx
proxy_buffer_size 16k;
proxy_buffers 4 16k;
proxy_busy_buffers_size 16k;
```

Without this, nginx's default 4k buffer will reject responses from the Kobo sync endpoint with "upstream sent too big header".

**5. Update config/kobbler.toml**

```toml
public_proxy_url = "https://kobbler.example.com/kobo"
```

Then restart the API: `docker compose restart api`

### Separate Subdomain Configuration

Create two proxy hosts:

**Proxy Host 1: Web/API** (`kobbler.example.com`)
- Forward Hostname/IP: `web` (or container IP)
- Forward Port: `80`
- Websockets Support: ON

**Proxy Host 2: Kobo Sync** (`kobo.example.com`)
- Forward Hostname/IP: `api` (or container IP) - **no trailing slash**
- Forward Port: `6767`

## Traefik Setup

Example `docker-compose.yml` labels:

```yaml
services:
  web:
    labels:
      - "traefik.enable=true"
      - "traefik.http.routers.kobbler-web.rule=Host(`kobbler.example.com`) && !PathPrefix(`/kobo`)"
      - "traefik.http.routers.kobbler-web.entrypoints=websecure"
      - "traefik.http.routers.kobbler-web.tls.certresolver=letsencrypt"
      - "traefik.http.services.kobbler-web.loadbalancer.server.port=80"

  api:
    labels:
      - "traefik.enable=true"
      # Kobo sync routes
      - "traefik.http.routers.kobbler-kobo.rule=Host(`kobbler.example.com`) && PathPrefix(`/kobo`)"
      - "traefik.http.routers.kobbler-kobo.entrypoints=websecure"
      - "traefik.http.routers.kobbler-kobo.tls.certresolver=letsencrypt"
      - "traefik.http.services.kobbler-kobo.loadbalancer.server.port=6767"
      - "traefik.http.middlewares.kobo-strip.stripprefix.prefixes=/kobo"
      - "traefik.http.routers.kobbler-kobo.middlewares=kobo-strip"
```

## Caddy Setup

Example Caddyfile:

```caddyfile
kobbler.example.com {
    # Kobo sync - strip /kobo prefix and forward to proxy port
    handle_path /kobo/* {
        reverse_proxy api:6767
    }

    # Everything else goes to web (which handles /api internally)
    handle {
        reverse_proxy web:80
    }
}
```

## Kobo Device Configuration

After setting up your reverse proxy, configure your Kobo device:

1. Connect your Kobo via USB
2. Edit `.kobo/Kobo/Kobo eReader.conf`
3. Add or modify the `[OneStoreServices]` section:

```ini
[OneStoreServices]
api_endpoint=https://kobbler.example.com/kobo/{device_token}
```

Replace `{device_token}` with the token shown in the Kobbler Devices page.

## Troubleshooting

### Kobo can't sync

1. Verify `public_proxy_url` in `config/kobbler.toml` matches your external URL exactly
2. Check that the `/kobo/` path is being proxied to port 6767
3. Test the API is reachable: `curl https://kobbler.example.com/health` (should print `ok`)
4. Check Kobbler logs: `docker compose logs api`

### WebSocket connection fails

1. Ensure your reverse proxy has WebSocket support enabled
2. The web UI uses WebSockets for real-time updates (job progress, etc.)
3. Check that the `Upgrade` and `Connection` headers are being forwarded

### 502 Bad Gateway on Kobo sync

The Kobo sync protocol uses large HTTP headers (sync tokens). If nginx returns "502 Bad Gateway" with `upstream sent too big header` in the error log, increase the proxy buffer size. In Nginx Proxy Manager, add to the **Advanced** tab:

```nginx
proxy_buffer_size 16k;
proxy_buffers 4 16k;
proxy_busy_buffers_size 16k;
```

For plain nginx, add these directives to the relevant `location` block. Check the error log to confirm: `docker exec <npm-container> cat /data/logs/proxy-host-*_error.log | tail -20`

### Large file transfers fail

Kobbler downloads ebook files from Kobload via streaming HTTP. Large files (audiobooks, comics) may time out or be rejected by reverse proxies with default settings.

Add this custom Nginx configuration in your reverse proxy (e.g., Nginx Proxy Manager → Advanced tab):

```nginx
client_max_body_size 0;          # Disable body size limit
proxy_request_buffering off;     # Stream request body directly
proxy_buffering off;             # Stream response body directly
proxy_read_timeout 86400s;       # 24h timeout for large transfers
proxy_send_timeout 86400s;
```

The Kobbler API allows up to 200MB for incoming requests by default (`HTTP_MAX_BODY_BYTES` env var). File downloads from Kobload are streamed and bypass this limit.

### SSL certificate issues

1. Ensure your reverse proxy is terminating SSL
2. Verify the `X-Forwarded-Proto` header is being set to `https`
3. Check that Force SSL is enabled in your proxy config
