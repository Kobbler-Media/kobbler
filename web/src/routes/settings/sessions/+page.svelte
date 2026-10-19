<script lang="ts">
  import { onMount } from 'svelte';
  import { getAuthState } from '$lib/stores/auth.svelte';
  import { showMessage } from '$lib/context/app.svelte';
  import * as api from '$lib/api/client';
  import MetaLine from '$lib/components/shared/MetaLine.svelte';
  import type { SessionSummary } from '$lib/types';

  const auth = getAuthState();

  let sessions = $state<SessionSummary[]>([]);
  let status = $state<'loading' | 'ready' | 'error'>('loading');
  let error = $state('');
  let showEveryone = $state(false);
  let busyId = $state<string | null>(null);
  let confirmId = $state<string | null>(null);

  const load = async () => {
    status = 'loading';
    try {
      sessions = await api.getSessions(showEveryone && auth.isAdmin);
      status = 'ready';
    } catch (e) {
      error = e instanceof Error ? e.message : 'Failed to load sessions';
      status = 'error';
    }
  };

  onMount(load);

  const toggleScope = async () => {
    showEveryone = !showEveryone;
    await load();
  };

  const revoke = async (session: SessionSummary) => {
    busyId = session.id;
    try {
      await api.revokeSession(session.id);
      showMessage(session.is_current ? 'Signed out' : 'Session revoked');
      if (session.is_current) {
        window.location.href = '/';
        return;
      }
      await load();
    } catch {
      showMessage('Revoke failed');
    } finally {
      busyId = null;
      confirmId = null;
    }
  };

  const owner = (s: SessionSummary) => s.display_name?.trim() || s.email;

  const label = (s: SessionSummary) => s.device_name?.trim() || 'Unnamed device';

  const kindLabel = (kind: string | null) => {
    switch (kind) {
      case 'android':
        return 'Android';
      case 'androidtv':
        return 'Android TV';
      case 'desktop':
        return 'Desktop app';
      case 'web':
        return 'Browser';
      default:
        return kind || 'Unknown client';
    }
  };

  const relative = (iso: string | null) => {
    if (!iso) return 'never';
    const delta = Date.now() - new Date(iso).getTime();
    const days = Math.floor(delta / 86_400_000);
    if (days > 1) return `${days} days ago`;
    if (days === 1) return 'yesterday';
    const hours = Math.floor(delta / 3_600_000);
    if (hours >= 1) return `${hours}h ago`;
    const mins = Math.floor(delta / 60_000);
    return mins >= 1 ? `${mins}m ago` : 'just now';
  };

  const expiryLabel = (iso: string) => {
    const days = Math.ceil((new Date(iso).getTime() - Date.now()) / 86_400_000);
    if (days <= 0) return 'expired';
    if (days <= 7) return `expires in ${days} day${days === 1 ? '' : 's'}`;
    return `expires ${new Date(iso).toLocaleDateString()}`;
  };

  const expiringSoon = (iso: string) => new Date(iso).getTime() - Date.now() < 7 * 86_400_000;
</script>

<svelte:head>
  <title>Sessions — Kobbler</title>
</svelte:head>

<div class="sessions-section">
  <div class="section-header">
    <h2>Linked Devices</h2>
    <div class="header-actions">
      <span class="muted">{sessions.length} active</span>
      {#if auth.isAdmin}
        <button class="btn alt small" type="button" onclick={toggleScope}>
          {showEveryone ? 'Show only mine' : 'Show all users'}
        </button>
      {/if}
    </div>
  </div>

  <p class="muted intro">
    Every signed-in app, TV and browser holds its own key. Revoking one signs that device out
    immediately; the others stay linked. Keys are never shown here — only that they exist.
  </p>

  {#if status === 'loading'}
    <div class="muted">Loading sessions…</div>
  {:else if status === 'error'}
    <div class="error">{error}</div>
  {:else if sessions.length === 0}
    <div class="empty-state">
      <h3>No linked devices</h3>
      <p class="muted">Pair a device from the Kobbler app to see it listed here.</p>
    </div>
  {:else}
    <div class="session-list">
      {#each sessions as session (session.id)}
        <div class="session-row" class:current={session.is_current}>
          <div class="session-info">
            <div class="session-title">
              {label(session)}
              {#if session.is_current}<span class="pill current-pill">This device</span>{/if}
              {#if expiringSoon(session.expires_at)}<span class="pill warn-pill">Expiring</span
                >{/if}
            </div>
            <div class="session-meta muted">
              <MetaLine
                items={[
                  kindLabel(session.client_kind),
                  showEveryone && auth.isAdmin && owner(session),
                  `last used ${relative(session.last_used_at)}`,
                  expiryLabel(session.expires_at)
                ]}
              />
            </div>
            <div class="session-meta muted">
              <MetaLine
                items={[
                  `Linked ${new Date(session.created_at).toLocaleDateString()}`,
                  session.created_by_email && `provisioned by ${session.created_by_email}`
                ]}
              />
            </div>
          </div>
          <div class="session-actions">
            {#if confirmId === session.id}
              <button
                class="btn btn-danger small"
                type="button"
                onclick={() => revoke(session)}
                disabled={busyId === session.id}
              >
                {session.is_current ? 'Sign out' : 'Confirm'}
              </button>
              <button class="btn alt small" type="button" onclick={() => (confirmId = null)}>
                Cancel
              </button>
            {:else}
              <button
                class="btn btn-danger small"
                type="button"
                onclick={() => (confirmId = session.id)}
                disabled={busyId !== null}
              >
                Revoke
              </button>
            {/if}
          </div>
        </div>
      {/each}
    </div>
  {/if}
</div>

<style>
  .sessions-section {
    display: grid;
    gap: 16px;
  }

  .section-header {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
  }

  .section-header h2 {
    margin: 0;
    font-size: 18px;
  }

  .header-actions {
    display: flex;
    align-items: center;
    gap: 12px;
  }

  .intro {
    margin: 0;
    font-size: 13px;
    max-width: 60ch;
  }

  .session-list {
    display: grid;
    gap: 8px;
  }

  .session-row {
    display: flex;
    align-items: flex-start;
    justify-content: space-between;
    gap: 12px;
    padding: 12px 16px;
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 12px;
  }

  .session-row.current {
    border-color: var(--accent);
  }

  .session-info {
    flex: 1;
    min-width: 0;
  }

  .session-title {
    display: flex;
    align-items: center;
    flex-wrap: wrap;
    gap: 8px;
    font-weight: 600;
    font-size: 14px;
    margin-bottom: 4px;
  }

  .pill {
    font-size: 10px;
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.04em;
    padding: 2px 8px;
    border-radius: 999px;
  }

  .current-pill {
    background: color-mix(in srgb, var(--accent) 18%, transparent);
    color: var(--accent);
  }

  .warn-pill {
    background: color-mix(in srgb, var(--danger) 18%, transparent);
    color: var(--danger);
  }

  .session-meta {
    font-size: 11px;
    margin-top: 4px;
  }

  .session-actions {
    display: flex;
    gap: 8px;
    flex-shrink: 0;
  }

  .btn {
    padding: 8px 16px;
    border-radius: 12px;
    border: none;
    background: var(--accent);
    color: white;
    font-weight: 600;
    font-size: 13px;
    cursor: pointer;
  }

  .btn:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .btn.alt {
    background: var(--card);
    border: 1px solid var(--ring);
    color: var(--ink);
  }

  .btn.small {
    padding: 6px 10px;
    font-size: 12px;
  }

  .btn-danger {
    background: var(--danger);
    color: white;
  }

  .empty-state {
    padding: 32px;
    text-align: center;
    background: var(--card);
    border: 1px dashed var(--ring);
    border-radius: 16px;
  }

  .empty-state h3 {
    margin: 0 0 8px;
    font-size: 16px;
  }

  .empty-state p {
    margin: 0;
  }

  .error {
    padding: 16px;
    text-align: center;
    border-radius: 12px;
    border: 1px solid var(--danger);
    background: var(--card);
    color: var(--danger);
  }
</style>
