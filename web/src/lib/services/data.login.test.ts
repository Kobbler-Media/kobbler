import { afterEach, describe, expect, it, vi } from 'vitest';
import { login } from './data.svelte';
import { clearAuth, getAuthState } from '#lib/stores/auth.svelte.js';

const me = {
  user_id: 'u1',
  email: 'kid@example.com',
  role: 'viewer',
  avatar_url: '/api/v1/admin/users/u1/avatar',
  has_pin: true,
  is_kids: true,
  timezone: 'Europe/London',
  cast_receiver_app_id: 'CAFE1234'
};

function stubServer(loginStatus = 200) {
  const fetch = vi.fn(async (input: string | URL | Request) => {
    const url = String(input);
    const json = (status: number, body: unknown) =>
      new Response(JSON.stringify(body), {
        status,
        headers: { 'content-type': 'application/json' }
      });
    if (url.endsWith('/api/v1/admin/login')) {
      return loginStatus === 200
        ? json(200, { token: 't', role: 'viewer', email: me.email })
        : new Response('invalid credentials', { status: loginStatus });
    }
    if (url.endsWith('/api/v1/admin/me')) return json(200, me);
    return json(200, {});
  });
  vi.stubGlobal('fetch', fetch);
  return fetch;
}

afterEach(() => {
  vi.unstubAllGlobals();
  clearAuth();
});

describe('login', () => {
  it('loads the full profile, avatar included, before the first render', async () => {
    stubServer();
    await login(me.email, 'pw');
    const auth = getAuthState();
    expect(auth).toMatchObject({
      isAuthed: true,
      email: me.email,
      role: 'viewer',
      avatarUrl: me.avatar_url,
      hasPin: true,
      isKids: true,
      timezone: me.timezone,
      castReceiverAppId: me.cast_receiver_app_id
    });
  });

  it('rejects bad credentials with the server message and stays signed out', async () => {
    stubServer(401);
    await expect(login(me.email, 'nope')).rejects.toThrow('invalid credentials');
    expect(getAuthState().isAuthed).toBe(false);
  });
});
