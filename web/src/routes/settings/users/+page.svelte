<script lang="ts">
  import { onMount } from 'svelte';
  import { getLibraryState } from '$lib/stores/library.svelte';
  import { getDevicesState } from '$lib/stores/devices.svelte';
  import { loadUsers, showMessage } from '$lib/context/app.svelte';
  import { getAuthState } from '$lib/stores/auth.svelte';
  import { DeleteConfirmModal, SearchSelect, type SearchSelectItem } from '@koblibs/ui';
  import MetaLine from '$lib/components/shared/MetaLine.svelte';
  import * as api from '$lib/api/client';

  const auth = getAuthState();

  const zoneItems: SearchSelectItem[] = [
    { value: '', label: 'Server default' },
    ...(typeof Intl.supportedValuesOf === 'function'
      ? Intl.supportedValuesOf('timeZone').map((z) => ({ value: z, label: z }))
      : [])
  ];

  const roleItems: SearchSelectItem[] = [
    { value: 'admin', label: 'Admin' },
    { value: 'contributor', label: 'Contributor' },
    { value: 'viewer', label: 'Viewer' }
  ];

  // Store state
  const library = getLibraryState();
  const devicesStore = getDevicesState();

  // Derived state
  const users = $derived(devicesStore.users);
  const loadState = $derived(library.loadState);

  // Local state
  let loading = $state(false);
  let showNewUserForm = $state(false);
  let newUserEmail = $state('');
  let newUserName = $state('');
  let newUserPassword = $state('');
  let newUserRole = $state('viewer');
  let newUserKids = $state(false);
  let editingUserId = $state<string | null>(null);
  let editingUserEmail = $state('');
  let editingUserName = $state('');
  let editingUserTimezone = $state('');
  let editingUserPassword = $state('');
  let editingUserRole = $state('');
  let editingUserKids = $state(false);
  let deletingUserId = $state<string | null>(null);
  let deletingUserEmail = $state('');
  let deleting = $state(false);

  onMount(async () => {
    await loadUsers();
  });

  const handleCreateUser = async () => {
    if (!newUserEmail || !newUserPassword) {
      showMessage('Email and password required');
      return;
    }
    loading = true;
    try {
      await api.createUser(newUserEmail, newUserPassword, newUserRole, newUserName, newUserKids);
      showMessage('User created');
      await loadUsers();
      newUserEmail = '';
      newUserName = '';
      newUserPassword = '';
      newUserRole = 'viewer';
      newUserKids = false;
      showNewUserForm = false;
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Create user failed');
    } finally {
      loading = false;
    }
  };

  const handleDeleteUser = async () => {
    if (!deletingUserId) return;
    deleting = true;
    try {
      await api.deleteUser(deletingUserId);
      showMessage('User deleted');
      await loadUsers();
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Delete user failed');
    } finally {
      deleting = false;
      deletingUserId = null;
      deletingUserEmail = '';
    }
  };

  const handleUpdateUser = async (userId: string) => {
    const updates: {
      email?: string;
      password?: string;
      role?: string;
      display_name?: string;
      timezone?: string;
      is_kids?: boolean;
    } = {};
    if (editingUserEmail) updates.email = editingUserEmail;
    if (editingUserPassword) updates.password = editingUserPassword;
    if (editingUserRole) updates.role = editingUserRole;
    // Sent even when blank: that is how a name or a zone is cleared.
    updates.display_name = editingUserName;
    updates.timezone = editingUserTimezone;
    updates.is_kids = editingUserKids;
    loading = true;
    try {
      await api.updateUser(userId, updates);
      showMessage('User updated');
      await loadUsers();
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Update user failed');
    } finally {
      loading = false;
      editingUserId = null;
      editingUserEmail = '';
      editingUserName = '';
      editingUserTimezone = '';
      editingUserPassword = '';
      editingUserRole = '';
    }
  };
</script>

<svelte:head>
  <title>Users — Kobbler</title>
</svelte:head>

{#if !auth.isAdmin}
  <div class="permission-denied">
    <h2>Access Denied</h2>
    <p>You don't have permission to view this page.</p>
  </div>
{:else}
  <div class="users-section">
    <div class="section-header">
      <h2>Users</h2>
      <div class="section-row">
        <span class="muted">{users.length} users</span>
        <button class="btn" type="button" onclick={() => (showNewUserForm = !showNewUserForm)}>
          {showNewUserForm ? 'Cancel' : 'Create User'}
        </button>
      </div>
    </div>

    {#if showNewUserForm}
      <div class="new-user-form card">
        <h3>New user</h3>
        <label>
          Email
          <input placeholder="user@example.com" bind:value={newUserEmail} />
        </label>
        <label>
          Display name
          <input placeholder="Shown instead of the email address" bind:value={newUserName} />
        </label>
        <label>
          Password
          <input type="password" bind:value={newUserPassword} />
        </label>
        <div class="form-field-label">Role</div>
        <SearchSelect
          id="new-user-role"
          items={roleItems}
          value={newUserRole}
          onchange={(v) => {
            newUserRole = v;
          }}
        />
        <label class="check-field">
          <input type="checkbox" bind:checked={newUserKids} />
          <span>
            Kids profile
            <span class="muted">
              Only titles rated PG-13 / TV-14 and under, or allowed by hand. No Live TV, IPTV or
              chat.
            </span>
          </span>
        </label>
        <button class="btn" type="button" onclick={handleCreateUser} disabled={loading}>
          Create user
        </button>
      </div>
    {/if}

    {#if loadState.users.status === 'loading'}
      <div class="muted">Loading users…</div>
    {:else if loadState.users.status === 'error'}
      <div class="error">{loadState.users.error}</div>
    {:else if users.length === 0}
      <div class="empty-state">
        <h3>No users yet</h3>
        <p class="muted">Create a user to get started.</p>
      </div>
    {:else}
      <div class="user-list">
        {#each users as user (user.id)}
          <div class="user-row" class:editing={editingUserId === user.id}>
            {#if editingUserId === user.id}
              <div class="user-edit-form">
                <label>
                  Email
                  <input type="email" placeholder={user.email} bind:value={editingUserEmail} />
                </label>
                <label>
                  Display name
                  <input placeholder="Leave blank to show the email" bind:value={editingUserName} />
                </label>
                <label>
                  New Password
                  <input
                    type="password"
                    placeholder="Leave blank to keep current"
                    bind:value={editingUserPassword}
                  />
                </label>
                <div class="form-field-label">Timezone</div>
                <SearchSelect
                  id="edit-user-timezone"
                  items={zoneItems}
                  value={editingUserTimezone}
                  onchange={(v) => {
                    editingUserTimezone = v;
                  }}
                />
                <div class="form-field-label">Role</div>
                <SearchSelect
                  id="edit-user-role"
                  items={roleItems}
                  value={editingUserRole}
                  onchange={(v) => {
                    editingUserRole = v;
                  }}
                />
                <label class="check-field">
                  <input type="checkbox" bind:checked={editingUserKids} />
                  <span>
                    Kids profile
                    <span class="muted">
                      Only titles rated PG-13 / TV-14 and under, or allowed by hand. No Live TV,
                      IPTV or chat.
                    </span>
                  </span>
                </label>
                <div class="edit-buttons">
                  <button
                    class="btn small"
                    type="button"
                    onclick={() => handleUpdateUser(user.id)}
                    disabled={loading}
                  >
                    Save
                  </button>
                  <button
                    class="btn alt small"
                    type="button"
                    onclick={() => {
                      editingUserId = null;
                      editingUserEmail = '';
                      editingUserName = '';
                      editingUserTimezone = '';
                      editingUserPassword = '';
                    }}
                  >
                    Cancel
                  </button>
                </div>
              </div>
            {:else}
              <div class="user-info">
                <div class="user-email">
                  {user.display_name || user.email}
                  <span
                    class="role-badge"
                    class:admin={user.role === 'admin'}
                    class:contributor={user.role === 'contributor'}>{user.role}</span
                  >
                  {#if user.is_kids}<span class="role-badge kids">Kids</span>{/if}
                </div>
                {#if user.display_name || user.timezone}
                  <div class="user-sub muted">
                    <MetaLine items={[user.display_name && user.email, user.timezone]} />
                  </div>
                {/if}
              </div>
              <div class="user-actions">
                <button
                  class="btn alt small"
                  type="button"
                  onclick={() => {
                    editingUserId = user.id;
                    editingUserEmail = '';
                    editingUserName = user.display_name ?? '';
                    editingUserTimezone = user.timezone ?? '';
                    editingUserPassword = '';
                    editingUserRole = user.role;
                    editingUserKids = user.is_kids;
                  }}
                >
                  Edit
                </button>
                {#if user.email !== auth.email}
                  <button
                    class="btn danger small"
                    type="button"
                    onclick={() => {
                      deletingUserId = user.id;
                      deletingUserEmail = user.email;
                    }}
                  >
                    Delete
                  </button>
                {/if}
              </div>
            {/if}
          </div>
        {/each}
      </div>
    {/if}
  </div>
{/if}

{#if deletingUserId}
  <DeleteConfirmModal
    title="Delete User"
    message={`Are you sure you want to delete "${deletingUserEmail}"? This action cannot be undone.`}
    confirmLabel={deleting ? 'Deleting...' : 'Delete User'}
    loading={deleting}
    onCancel={() => {
      deletingUserId = null;
      deletingUserEmail = '';
    }}
    onConfirm={handleDeleteUser}
  />
{/if}

<style>
  .users-section {
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

  .card {
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 16px;
    padding: 16px;
  }

  .new-user-form {
    display: grid;
    gap: 12px;
  }

  .new-user-form h3 {
    margin: 0 0 8px;
    font-size: 16px;
  }

  .new-user-form label {
    display: grid;
    gap: 6px;
    font-size: 12px;
    color: var(--muted);
  }

  .new-user-form input {
    padding: 8px 10px;
    border-radius: 10px;
    border: 1px solid var(--ring);
    background: transparent;
    color: var(--ink);
    font-size: 13px;
  }

  .user-list {
    display: grid;
    gap: 8px;
  }

  .user-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 12px 16px;
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 12px;
  }

  .user-row.editing {
    flex-direction: column;
    align-items: stretch;
  }

  .user-edit-form {
    display: grid;
    gap: 12px;
  }

  .user-edit-form label {
    display: grid;
    gap: 6px;
    font-size: 12px;
    color: var(--muted);
  }

  .user-edit-form input {
    padding: 8px 10px;
    border-radius: 10px;
    border: 1px solid var(--ring);
    background: transparent;
    color: var(--ink);
    font-size: 13px;
  }

  .edit-buttons {
    display: flex;
    gap: 8px;
  }

  .user-info {
    flex: 1;
  }

  .user-sub {
    font-size: 0.85em;
  }

  .user-email {
    font-weight: 500;
  }

  .user-actions {
    display: flex;
    gap: 8px;
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

  .role-badge {
    font-size: 11px;
    padding: 2px 8px;
    border-radius: 6px;
    background: var(--ring);
    color: var(--muted);
    margin-left: 8px;
    text-transform: capitalize;
  }

  .role-badge.admin {
    background: var(--accent);
    color: white;
  }

  .role-badge.contributor {
    background: var(--accent-2, #22c55e);
    color: white;
  }

  .role-badge.kids {
    background: var(--accent-3, #f59e0b);
    color: white;
  }

  .new-user-form .check-field,
  .user-edit-form .check-field {
    display: flex;
    align-items: flex-start;
    gap: 10px;
    font-size: 13px;
    color: var(--ink);
  }

  .check-field input {
    margin-top: 3px;
  }

  .check-field .muted {
    display: block;
    font-size: 12px;
  }

  .form-field-label {
    font-size: 12px;
    color: var(--muted);
  }
</style>
