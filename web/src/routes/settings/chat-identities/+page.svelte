<script lang="ts">
  import { onMount } from 'svelte';
  import { getDevicesState } from '$lib/stores/devices.svelte';
  import { loadUsers, showMessage } from '$lib/context/app.svelte';
  import { getAuthState } from '$lib/stores/auth.svelte';
  import { DeleteConfirmModal, SearchSelect, type SearchSelectItem } from '@koblibs/ui';
  import DiscordSetup from '$lib/components/chat/DiscordSetup.svelte';
  import GoogleChatSetup from '$lib/components/chat/GoogleChatSetup.svelte';
  import * as api from '$lib/api/client';
  import MetaLine from '$lib/components/shared/MetaLine.svelte';
  import type { ChatIdentitiesResponse, ChatIdentity, ChatPendingPairing } from '$lib/types';

  const auth = getAuthState();
  const devicesStore = getDevicesState();
  const users = $derived(devicesStore.users);

  const ceilingItems: SearchSelectItem[] = [
    { value: 'viewer', label: 'Viewer — ask anything, change nothing' },
    { value: 'contributor', label: 'Contributor — can download and list things' },
    { value: 'admin', label: 'Admin — no ceiling' }
  ];

  const userItems = $derived<SearchSelectItem[]>(
    users.map((u) => ({ value: u.id, label: u.display_name || u.email }))
  );

  let status = $state<'loading' | 'error' | 'ready'>('loading');
  let error = $state('');
  let data = $state<ChatIdentitiesResponse>({
    identities: [],
    pending_pairings: [],
    transports: []
  });

  let busy = $state(false);
  let showNewForm = $state(false);
  let newTransport = $state('');
  let newUserId = $state('');
  let newAddress = $state('');
  let newMaxRole = $state('contributor');

  let editingId = $state<string | null>(null);
  let editAddress = $state('');
  let editUserId = $state('');
  let editMaxRole = $state('');
  let editEnabled = $state(true);

  let approvingId = $state<string | null>(null);
  let approveUserId = $state('');
  let approveMaxRole = $state('contributor');

  let deletingId = $state<string | null>(null);
  let deletingLabel = $state('');
  let deleting = $state(false);

  const identityLabel = (identity: ChatIdentity) =>
    identity.address || identity.external_id || identity.id;

  const statusLabel = (identity: ChatIdentity) => {
    if (identity.status === 'disabled') return 'Disabled';
    if (identity.status === 'awaiting_first_message') return 'Waiting for first message';
    return 'Active';
  };

  const formatWhen = (value?: string | null) => {
    if (!value) return 'never';
    const when = new Date(value);
    return Number.isNaN(when.getTime()) ? 'never' : when.toLocaleString();
  };

  const minutesLeft = (pairing: ChatPendingPairing) => {
    const ms = new Date(pairing.expires_at).getTime() - Date.now();
    if (Number.isNaN(ms) || ms <= 0) return 'expired';
    return `${Math.max(1, Math.round(ms / 60000))} min left`;
  };

  const load = async () => {
    try {
      data = await api.getChatIdentities();
      status = 'ready';
    } catch (e) {
      error = e instanceof Error ? e.message : 'Failed to load chat identities';
      status = 'error';
    }
  };

  const hasGoogleChat = $derived(data.transports.some((t) => t.id === 'google_chat'));
  const hasDiscord = $derived(data.transports.some((t) => t.id === 'discord'));

  // Discord has no human-readable address — a username can change, the numeric id
  // cannot — so the id is what gets typed here. Saying so in the field is the
  // difference between a binding that works and one that looks right and never matches.
  const ID_ONLY_TRANSPORTS = new Set(['discord']);
  const addressHint = (transport: string) =>
    ID_ONLY_TRANSPORTS.has(transport)
      ? {
          placeholder: '461291837402193920',
          help: 'Their Discord user ID — right-click their name with Developer Mode on and Copy User ID. Or let them message the bot and approve the pairing code it gives them.'
        }
      : { placeholder: 'her@example.com', help: '' };
  const newAddressHint = $derived(addressHint(newTransport));

  onMount(async () => {
    await Promise.all([loadUsers(), load()]);
    newTransport = data.transports[0]?.id ?? '';
  });

  const handleCreate = async () => {
    if (!newTransport || !newUserId || !newAddress.trim()) {
      showMessage('Pick a user and type their chat address');
      return;
    }
    busy = true;
    try {
      await api.createChatIdentity({
        transport: newTransport,
        user_id: newUserId,
        address: newAddress.trim(),
        max_role: newMaxRole
      });
      showMessage('Chat identity added');
      await load();
      newUserId = '';
      newAddress = '';
      newMaxRole = 'contributor';
      showNewForm = false;
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Could not add that chat identity');
    } finally {
      busy = false;
    }
  };

  const startEdit = (identity: ChatIdentity) => {
    editingId = identity.id;
    editAddress = identity.address ?? '';
    editUserId = identity.user_id;
    editMaxRole = identity.max_role;
    editEnabled = identity.enabled;
  };

  const handleUpdate = async (identityId: string) => {
    busy = true;
    try {
      await api.updateChatIdentity(identityId, {
        address: editAddress.trim() ? editAddress.trim() : null,
        user_id: editUserId,
        max_role: editMaxRole,
        enabled: editEnabled
      });
      showMessage('Chat identity updated');
      await load();
      editingId = null;
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Could not update that chat identity');
    } finally {
      busy = false;
    }
  };

  const handleDelete = async () => {
    if (!deletingId) return;
    deleting = true;
    try {
      await api.deleteChatIdentity(deletingId);
      showMessage('Chat identity removed');
      await load();
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Could not remove that chat identity');
    } finally {
      deleting = false;
      deletingId = null;
      deletingLabel = '';
    }
  };

  const handleApprove = async (pairing: ChatPendingPairing) => {
    if (!approveUserId) {
      showMessage('Pick the kobbler user this belongs to');
      return;
    }
    busy = true;
    try {
      await api.approveChatPairing(pairing.id, {
        user_id: approveUserId,
        max_role: approveMaxRole
      });
      showMessage('Paired');
      await load();
      approvingId = null;
      approveUserId = '';
      approveMaxRole = 'contributor';
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Could not approve that code');
    } finally {
      busy = false;
    }
  };

  const handleReject = async (pairing: ChatPendingPairing) => {
    busy = true;
    try {
      await api.rejectChatPairing(pairing.id);
      showMessage('Request dismissed');
      await load();
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Could not dismiss that request');
    } finally {
      busy = false;
    }
  };
</script>

<svelte:head>
  <title>Chat Identities — Kobbler</title>
</svelte:head>

{#if !auth.isAdmin}
  <div class="permission-denied">
    <h2>Access Denied</h2>
    <p>You don't have permission to view this page.</p>
  </div>
{:else}
  <div class="page">
    {#if data.pending_pairings.length > 0}
      <section class="section">
        <div class="section-header">
          <h2>Waiting for approval</h2>
          <span class="muted">
            Someone messaged the bot from an address we can't see. Check the code matches what they
            were shown before approving.
          </span>
        </div>
        <div class="list">
          {#each data.pending_pairings as pairing (pairing.id)}
            <div class="row pending">
              <div class="row-main">
                <div class="row-title">
                  <code class="code">{pairing.code}</code>
                  <span class="who">{pairing.display_name || pairing.external_id}</span>
                  <span class="badge">{minutesLeft(pairing)}</span>
                </div>
                <div class="row-sub muted">
                  <MetaLine items={[pairing.transport, pairing.external_id, pairing.space]} />
                </div>
              </div>
              {#if approvingId === pairing.id}
                <div class="approve-form">
                  <div class="form-field-label">Kobbler user</div>
                  <SearchSelect
                    id={`approve-user-${pairing.id}`}
                    items={userItems}
                    value={approveUserId}
                    onchange={(v) => {
                      approveUserId = v;
                    }}
                  />
                  <div class="form-field-label">Ceiling</div>
                  <SearchSelect
                    id={`approve-role-${pairing.id}`}
                    items={ceilingItems}
                    value={approveMaxRole}
                    onchange={(v) => {
                      approveMaxRole = v;
                    }}
                  />
                  <div class="row-actions">
                    <button
                      class="btn small"
                      type="button"
                      disabled={busy}
                      onclick={() => handleApprove(pairing)}
                    >
                      Approve
                    </button>
                    <button
                      class="btn alt small"
                      type="button"
                      onclick={() => {
                        approvingId = null;
                        approveUserId = '';
                      }}
                    >
                      Cancel
                    </button>
                  </div>
                </div>
              {:else}
                <div class="row-actions">
                  <button
                    class="btn small"
                    type="button"
                    onclick={() => {
                      approvingId = pairing.id;
                      approveUserId = '';
                      approveMaxRole = 'contributor';
                    }}
                  >
                    Approve
                  </button>
                  <button
                    class="btn alt small"
                    type="button"
                    disabled={busy}
                    onclick={() => handleReject(pairing)}
                  >
                    Dismiss
                  </button>
                </div>
              {/if}
            </div>
          {/each}
        </div>
      </section>
    {/if}

    {#if hasGoogleChat}
      <GoogleChatSetup />
    {/if}

    {#if hasDiscord}
      <DiscordSetup />
    {/if}

    <section class="section">
      <div class="section-header">
        <h2>Chat Identities</h2>
        <div class="section-row">
          <span class="muted">{data.identities.length} bound</span>
          <button
            class="btn"
            type="button"
            disabled={data.transports.length === 0}
            onclick={() => (showNewForm = !showNewForm)}
          >
            {showNewForm ? 'Cancel' : 'Add Identity'}
          </button>
        </div>
      </div>

      <p class="muted intro">
        A chat address is a credential for an existing kobbler user, not a second account. The
        ceiling caps what chat can do — the role on the Users page is still the floor, so demoting
        someone there takes effect on their next message.
      </p>

      {#if showNewForm}
        <div class="card form">
          <h3>New identity</h3>
          {#if data.transports.length > 1}
            <div class="form-field-label">Transport</div>
            <SearchSelect
              id="new-identity-transport"
              items={data.transports.map((t) => ({ value: t.id, label: t.display_name }))}
              value={newTransport}
              onchange={(v) => {
                newTransport = v;
              }}
            />
          {/if}
          <div class="form-field-label">Kobbler user</div>
          <SearchSelect
            id="new-identity-user"
            items={userItems}
            value={newUserId}
            onchange={(v) => {
              newUserId = v;
            }}
          />
          <label>
            Chat address
            <input placeholder={newAddressHint.placeholder} bind:value={newAddress} />
          </label>
          {#if newAddressHint.help}
            <p class="field-help muted">{newAddressHint.help}</p>
          {/if}
          <div class="form-field-label">Ceiling</div>
          <SearchSelect
            id="new-identity-role"
            items={ceilingItems}
            value={newMaxRole}
            onchange={(v) => {
              newMaxRole = v;
            }}
          />
          <button class="btn" type="button" onclick={handleCreate} disabled={busy}>
            Add identity
          </button>
        </div>
      {/if}

      {#if status === 'loading'}
        <div class="muted">Loading chat identities…</div>
      {:else if status === 'error'}
        <div class="error">{error}</div>
      {:else if data.identities.length === 0}
        <div class="empty-state">
          <h3>Nobody is set up for chat yet</h3>
          <p class="muted">
            Add someone's chat address and bind it to their kobbler account to let them talk to the
            library.
          </p>
        </div>
      {:else}
        <div class="list">
          {#each data.identities as identity (identity.id)}
            <div class="row" class:editing={editingId === identity.id}>
              {#if editingId === identity.id}
                <div class="form">
                  <label>
                    Chat address
                    <input
                      placeholder={addressHint(identity.transport).placeholder}
                      bind:value={editAddress}
                    />
                  </label>
                  <div class="form-field-label">Kobbler user</div>
                  <SearchSelect
                    id={`edit-user-${identity.id}`}
                    items={userItems}
                    value={editUserId}
                    onchange={(v) => {
                      editUserId = v;
                    }}
                  />
                  <div class="form-field-label">Ceiling</div>
                  <SearchSelect
                    id={`edit-role-${identity.id}`}
                    items={ceilingItems}
                    value={editMaxRole}
                    onchange={(v) => {
                      editMaxRole = v;
                    }}
                  />
                  <label class="checkbox">
                    <input type="checkbox" bind:checked={editEnabled} />
                    Enabled
                  </label>
                  <div class="row-actions">
                    <button
                      class="btn small"
                      type="button"
                      disabled={busy}
                      onclick={() => handleUpdate(identity.id)}
                    >
                      Save
                    </button>
                    <button class="btn alt small" type="button" onclick={() => (editingId = null)}>
                      Cancel
                    </button>
                  </div>
                </div>
              {:else}
                <div class="row-main">
                  <div class="row-title">
                    {identityLabel(identity)}
                    <span
                      class="badge"
                      class:ok={identity.status === 'active'}
                      class:warn={identity.status === 'awaiting_first_message'}
                      class:off={identity.status === 'disabled'}
                    >
                      {statusLabel(identity)}
                    </span>
                    <span class="badge role">{identity.effective_role}</span>
                  </div>
                  <div class="row-sub muted">
                    <MetaLine
                      items={[
                        `${identity.user_display_name || identity.user_email} (${identity.user_role})`,
                        `ceiling ${identity.max_role}`,
                        `last seen ${formatWhen(identity.last_seen_at)}`
                      ]}
                    />
                  </div>
                  {#if !identity.external_id}
                    <div class="row-sub muted">
                      No stable id learned yet — this binding relies on the address staying the
                      same.
                    </div>
                  {/if}
                </div>
                <div class="row-actions">
                  <button class="btn alt small" type="button" onclick={() => startEdit(identity)}>
                    Edit
                  </button>
                  <button
                    class="btn danger small"
                    type="button"
                    onclick={() => {
                      deletingId = identity.id;
                      deletingLabel = identityLabel(identity);
                    }}
                  >
                    Delete
                  </button>
                </div>
              {/if}
            </div>
          {/each}
        </div>
      {/if}
    </section>
  </div>
{/if}

{#if deletingId}
  <DeleteConfirmModal
    title="Remove Chat Identity"
    message={`Remove "${deletingLabel}"? They will stop being able to talk to the library.`}
    confirmLabel={deleting ? 'Removing...' : 'Remove'}
    loading={deleting}
    onCancel={() => {
      deletingId = null;
      deletingLabel = '';
    }}
    onConfirm={handleDelete}
  />
{/if}

<style>
  .page {
    display: grid;
    gap: 24px;
  }

  .section {
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

  .section-row {
    display: flex;
    align-items: center;
    gap: 12px;
  }

  .intro {
    margin: 0;
    max-width: 68ch;
    font-size: 13px;
  }

  .card {
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 16px;
    padding: 16px;
  }

  .form {
    display: grid;
    gap: 12px;
  }

  .form h3 {
    margin: 0 0 8px;
    font-size: 16px;
  }

  .form label {
    display: grid;
    gap: 6px;
    font-size: 12px;
    color: var(--muted);
  }

  .form label.checkbox {
    grid-auto-flow: column;
    justify-content: start;
    align-items: center;
    gap: 8px;
  }

  .form input {
    padding: 8px 10px;
    border-radius: 10px;
    border: 1px solid var(--ring);
    background: transparent;
    color: var(--ink);
    font-size: 13px;
  }

  .list {
    display: grid;
    gap: 8px;
  }

  .row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 12px 16px;
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 12px;
  }

  .row.editing {
    flex-direction: column;
    align-items: stretch;
  }

  .row.pending {
    border-color: var(--accent);
    flex-wrap: wrap;
  }

  .row-main {
    flex: 1;
    min-width: 220px;
    display: grid;
    gap: 4px;
  }

  .row-title {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
    font-weight: 500;
  }

  .row-sub {
    font-size: 0.85em;
  }

  .row-actions {
    display: flex;
    gap: 8px;
  }

  .approve-form {
    display: grid;
    gap: 12px;
    min-width: 260px;
  }

  .code {
    font-family: var(--mono, ui-monospace, monospace);
    font-size: 15px;
    letter-spacing: 2px;
    padding: 4px 10px;
    border-radius: 8px;
    background: var(--ring);
    color: var(--ink);
  }

  .who {
    font-weight: 500;
  }

  .badge {
    font-size: 11px;
    padding: 2px 8px;
    border-radius: 6px;
    background: var(--ring);
    color: var(--muted);
    text-transform: capitalize;
  }

  .badge.ok {
    background: var(--accent-2, #22c55e);
    color: white;
  }

  .badge.warn {
    background: var(--warning, #f59e0b);
    color: white;
  }

  .badge.off {
    background: var(--danger, #ef4444);
    color: white;
  }

  .badge.role {
    background: var(--accent);
    color: white;
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

  .btn.danger {
    background: var(--danger, #ef4444);
    color: white;
  }

  .btn.small {
    padding: 6px 10px;
    font-size: 12px;
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
    margin: 0 auto;
    max-width: 50ch;
  }

  .error {
    padding: 16px;
    text-align: center;
    border-radius: 12px;
    border: 1px solid var(--danger);
    background: var(--card);
    color: var(--danger);
  }

  .permission-denied {
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    padding: 4rem 2rem;
    color: var(--muted);
    text-align: center;
  }

  .permission-denied h2 {
    font-size: 1.25rem;
    margin-bottom: 0.5rem;
    color: var(--ink);
  }

  .form-field-label {
    font-size: 12px;
    color: var(--muted);
  }

  .field-help {
    margin: -4px 0 0;
    max-width: 74ch;
    font-size: 12px;
    line-height: 1.5;
  }
</style>
