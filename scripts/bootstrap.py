#!/usr/bin/env python3
import argparse
import json
import sys
from urllib import request


def http_json(method, url, payload=None, token=None):
    data = None
    if payload is not None:
        data = json.dumps(payload).encode("utf-8")
    req = request.Request(url, data=data, method=method)
    if data is not None:
        req.add_header("Content-Type", "application/json")
    if token:
        req.add_header("Authorization", f"Bearer {token}")
    try:
        with request.urlopen(req) as resp:
            body = resp.read().decode("utf-8")
            return resp.status, json.loads(body) if body else None
    except Exception as exc:
        print(f"Request failed: {exc}", file=sys.stderr)
        sys.exit(1)


def main():
    parser = argparse.ArgumentParser(description="Bootstrap Kobbler admin/user/device/tokens")
    parser.add_argument("--base-url", required=True, help="API base URL, e.g. http://localhost:8080")
    parser.add_argument("--admin-email", required=True)
    parser.add_argument("--admin-password", required=True)
    parser.add_argument("--user-email", required=True)
    parser.add_argument("--user-password", required=True)
    parser.add_argument("--device-name", default="Kobo")
    args = parser.parse_args()

    status, login = http_json(
        "POST",
        f"{args.base_url}/api/v1/admin/login",
        {"email": args.admin_email, "password": args.admin_password},
    )
    if status != 200:
        print("Admin login failed", file=sys.stderr)
        sys.exit(1)

    token = login["token"]

    status, user = http_json(
        "POST",
        f"{args.base_url}/api/v1/admin/users",
        {"email": args.user_email, "password": args.user_password},
        token=token,
    )
    if status != 200:
        print("User creation failed", file=sys.stderr)
        sys.exit(1)

    user_id = user["user_id"]

    status, device = http_json(
        "POST",
        f"{args.base_url}/api/v1/admin/devices",
        {"user_id": user_id, "name": args.device_name},
        token=token,
    )
    if status != 200:
        print("Device creation failed", file=sys.stderr)
        sys.exit(1)

    status, ingest = http_json(
        "POST",
        f"{args.base_url}/api/v1/admin/ingest-tokens",
        None,
        token=token,
    )
    if status != 200:
        print("Ingest token creation failed", file=sys.stderr)
        sys.exit(1)

    print("\nBootstrap complete\n")
    print(f"User ID: {user_id}")
    print(f"Device ID: {device['device_id']}")
    print(f"Device Token: {device['device_token']}")
    print(f"Kobo Sync URL: {device['sync_url']}")
    print(f"Calibre Ingest Token: {ingest['token']}")
    print("\nDevice setup:")
    print("- On the Kobo, set api_endpoint in .kobo/Kobo/Kobo eReader.conf")
    print(f"  api_endpoint={device['sync_url']}")
    print("\nCalibre plugin setup:")
    print(f"- Base URL: {args.base_url}")
    print(f"- Token: {ingest['token']}")


if __name__ == "__main__":
    main()
