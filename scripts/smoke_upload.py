#!/usr/bin/env python3
import argparse
import json
import sys
import uuid
from pathlib import Path
from urllib import request


def api_call(method, url, token, data=None, content_type=None, raw=False):
    body = None
    if data is not None:
        if raw:
            body = data
        else:
            body = json.dumps(data).encode('utf-8')
    req = request.Request(url, data=body, method=method)
    req.add_header('Authorization', f'Bearer {token}')
    if content_type:
        req.add_header('Content-Type', content_type)
    with request.urlopen(req) as resp:
        if resp.status >= 400:
            raise RuntimeError(f'HTTP {resp.status} for {url}')
        return resp.read().decode('utf-8')


def main():
    parser = argparse.ArgumentParser(description='Smoke test upload to Kobbler')
    parser.add_argument('--base-url', required=True)
    parser.add_argument('--token', required=True)
    parser.add_argument('--kepub', required=True)
    parser.add_argument('--cover')
    parser.add_argument('--title', default='Smoke Test Book')
    args = parser.parse_args()

    base = args.base_url.rstrip('/')
    book_id = str(uuid.uuid4())

    api_call('PUT', f'{base}/api/v1/books/{book_id}', args.token, {
        'title': args.title,
        'authors': ['Smoke Tester'],
        'language': 'en'
    }, content_type='application/json')

    if args.cover:
        cover_bytes = Path(args.cover).read_bytes()
        api_call('PUT', f'{base}/api/v1/books/{book_id}/cover', args.token, cover_bytes,
                 content_type='image/jpeg', raw=True)

    kepub_bytes = Path(args.kepub).read_bytes()
    api_call('PUT', f'{base}/api/v1/books/{book_id}/files/KEPUB', args.token, kepub_bytes,
             content_type='application/octet-stream', raw=True)

    api_call('POST', f'{base}/api/v1/entitlements', args.token, {
        'book_id': book_id,
        'scope_type': 'ALL_USERS'
    }, content_type='application/json')

    print('Uploaded book_id:', book_id)


if __name__ == '__main__':
    try:
        main()
    except Exception as exc:
        print(f'Error: {exc}', file=sys.stderr)
        sys.exit(1)
