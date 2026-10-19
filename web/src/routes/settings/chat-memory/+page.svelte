<script lang="ts">
  import { onMount } from 'svelte';
  import { DeleteConfirmModal, SearchSelect, type SearchSelectItem } from '@koblibs/ui';
  import { showMessage } from '$lib/context/app.svelte';
  import { getAuthState } from '$lib/stores/auth.svelte';
  import * as api from '$lib/api/client';
  import MetaLine from '$lib/components/shared/MetaLine.svelte';
  import type {
    ChatFactKind,
    ChatMemoryResponse,
    ChatMemoryUser,
    ChatProfileFact
  } from '$lib/types';

  const auth = getAuthState();

  const KIND_ITEMS: SearchSelectItem[] = [
    { value: 'prefers', label: 'prefers — how you like things done' },
    { value: 'likes', label: 'likes — what you enjoy' },
    { value: 'avoids', label: 'avoids — what you would rather not' }
  ];

  const KIND_ORDER: ChatFactKind[] = ['prefers', 'likes', 'avoids'];

  let status = $state<'loading' | 'error' | 'ready'>('loading');
  let error = $state('');
  let data = $state<ChatMemoryResponse>({
    facts: [],
    summaries: [],
    max_facts: 20,
    kinds: KIND_ORDER
  });

  let busy = $state(false);

  let newKind = $state<ChatFactKind>('likes');
  let newValue = $state('');

  let editingId = $state<string | null>(null);
  let editKind = $state<ChatFactKind>('likes');
  let editValue = $state('');

  let deletingId = $state<string | null>(null);
  let deletingLabel = $state('');
  let deleting = $state(false);

  let confirmForget = $state(false);
  let forgetting = $state(false);

  let households = $state<ChatMemoryUser[]>([]);

  const grouped = $derived(
    KIND_ORDER.map((kind) => ({
      kind,
      facts: data.facts.filter((f) => f.kind === kind)
    })).filter((group) => group.facts.length > 0)
  );

  const atCap = $derived(data.facts.length >= data.max_facts);

  const formatWhen = (value?: string | null) => {
    if (!value) return '';
    const when = new Date(value);
    return Number.isNaN(when.getTime()) ? '' : when.toLocaleDateString();
  };

  const formatFull = (value?: string | null) => {
    if (!value) return 'still going';
    const when = new Date(value);
    return Number.isNaN(when.getTime()) ? 'still going' : when.toLocaleString();
  };

  const load = async () => {
    try {
      data = await api.getChatMemory();
      status = 'ready';
    } catch (e) {
      error = e instanceof Error ? e.message : 'Could not load what the assistant remembers';
      status = 'error';
    }
  };

  // Admins get counts and timestamps only — never anyone else's facts or
  // summaries. The server withholds them; this is not a display decision.
  const loadHousehold = async () => {
    if (!auth.isAdmin) return;
    try {
      households = await api.listChatMemoryUsers();
    } catch {
      households = [];
    }
  };

  onMount(async () => {
    await Promise.all([load(), loadHousehold()]);
  });

  const handleAdd = async () => {
    if (!newValue.trim()) {
      showMessage('Type what you want it to remember');
      return;
    }
    busy = true;
    try {
      await api.createChatFact({ kind: newKind, value: newValue.trim() });
      newValue = '';
      await load();
      showMessage('Added');
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Could not add that');
    } finally {
      busy = false;
    }
  };

  const startEdit = (fact: ChatProfileFact) => {
    editingId = fact.id;
    editKind = fact.kind;
    editValue = fact.value;
  };

  const handleSave = async () => {
    if (!editingId || !editValue.trim()) return;
    busy = true;
    try {
      await api.updateChatFact(editingId, { kind: editKind, value: editValue.trim() });
      editingId = null;
      await load();
      showMessage('Corrected');
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Could not save that');
    } finally {
      busy = false;
    }
  };

  const handleDelete = async () => {
    if (!deletingId) return;
    deleting = true;
    try {
      await api.deleteChatFact(deletingId);
      await load();
      showMessage('Forgotten');
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Could not remove that');
    } finally {
      deleting = false;
      deletingId = null;
      deletingLabel = '';
    }
  };

  const handleForget = async () => {
    forgetting = true;
    try {
      const result = await api.forgetChatMemory();
      await Promise.all([load(), loadHousehold()]);
      showMessage(
        `Forgotten — ${result.messages} messages, ${result.sessions} conversations, ${result.facts} facts`
      );
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Could not forget that');
    } finally {
      forgetting = false;
      confirmForget = false;
    }
  };
</script>

<svelte:head>
  <title>Chat Memory — Kobbler</title>
</svelte:head>

<div class="page">
  <section class="section">
    <div class="section-header">
      <h2>What the assistant remembers about you</h2>
    </div>
    <p class="intro muted">
      A short list the assistant carries into every conversation, picked up from things you have
      said. It is yours alone — nobody else can read it, not even an admin — and it is only useful
      if it is right, so correct anything that is not.
    </p>

    {#if status === 'loading'}
      <p class="muted">Loading…</p>
    {:else if status === 'error'}
      <p class="error">{error}</p>
    {:else}
      {#if grouped.length === 0}
        <div class="card empty">
          <p class="muted">
            Nothing yet. It picks things up as you talk to it, or you can add one below.
          </p>
        </div>
      {:else}
        <div class="list">
          {#each grouped as group (group.kind)}
            <div class="group">
              <div class="group-label">{group.kind}</div>
              {#each group.facts as fact (fact.id)}
                <div class="row">
                  {#if editingId === fact.id}
                    <div class="edit-form">
                      <SearchSelect
                        id={`kind-${fact.id}`}
                        items={KIND_ITEMS}
                        value={editKind}
                        onchange={(v) => {
                          editKind = v as ChatFactKind;
                        }}
                      />
                      <input
                        type="text"
                        bind:value={editValue}
                        maxlength="200"
                        aria-label="What it remembers"
                      />
                      <div class="row-actions">
                        <button
                          class="btn small"
                          type="button"
                          disabled={busy}
                          onclick={handleSave}
                        >
                          Save
                        </button>
                        <button
                          class="btn alt small"
                          type="button"
                          onclick={() => {
                            editingId = null;
                          }}
                        >
                          Cancel
                        </button>
                      </div>
                    </div>
                  {:else}
                    <div class="row-main">
                      <div class="row-title">{fact.value}</div>
                      {#if fact.created_at}
                        <div class="row-sub muted">learned {formatWhen(fact.created_at)}</div>
                      {/if}
                    </div>
                    <div class="row-actions">
                      <button class="btn alt small" type="button" onclick={() => startEdit(fact)}>
                        Edit
                      </button>
                      <button
                        class="btn danger small"
                        type="button"
                        onclick={() => {
                          deletingId = fact.id;
                          deletingLabel = fact.value;
                        }}
                      >
                        Delete
                      </button>
                    </div>
                  {/if}
                </div>
              {/each}
            </div>
          {/each}
        </div>
      {/if}

      <div class="card form">
        <h3>Add something</h3>
        <label for="new-kind">Kind</label>
        <SearchSelect
          id="new-kind"
          items={KIND_ITEMS}
          value={newKind}
          onchange={(v) => {
            newKind = v as ChatFactKind;
          }}
        />
        <label for="new-value">What it should remember</label>
        <input
          id="new-value"
          type="text"
          bind:value={newValue}
          maxlength="200"
          placeholder="70s horror, slow burns"
        />
        <div class="row-actions">
          <button class="btn small" type="button" disabled={busy || atCap} onclick={handleAdd}>
            Add
          </button>
          <span class="muted count">{data.facts.length} of {data.max_facts}</span>
        </div>
        {#if atCap}
          <p class="muted">
            That is as many as it keeps. Remove one to make room — a longer list stops being a
            profile and starts being a log.
          </p>
        {/if}
      </div>
    {/if}
  </section>

  {#if status === 'ready' && data.summaries.length > 0}
    <section class="section">
      <div class="section-header">
        <h2>Recent conversations</h2>
      </div>
      <p class="intro muted">
        A few lines each, written when a conversation goes quiet, so "did that ever finish?" works
        the next morning. Only conversations where something actually happened get one.
      </p>
      <div class="list">
        {#each data.summaries as session (session.id)}
          <div class="row">
            <div class="row-main">
              <div class="row-sub muted">
                <MetaLine
                  items={[session.transport, formatFull(session.closed_at ?? session.created_at)]}
                />
              </div>
              <div class="summary">{session.summary}</div>
            </div>
          </div>
        {/each}
      </div>
    </section>
  {/if}

  {#if auth.isAdmin && households.length > 0}
    <section class="section">
      <div class="section-header">
        <h2>Who is using the assistant</h2>
      </div>
      <p class="intro muted">
        How much and how recently, and nothing about what. Conversations and remembered facts belong
        to the person who had them — this page cannot show you anyone else's, and neither can the
        API behind it.
      </p>
      <div class="table-wrap">
        <table>
          <thead>
            <tr>
              <th>Person</th>
              <th>Conversations</th>
              <th>Messages</th>
              <th>Summaries</th>
              <th>Facts</th>
              <th>Last seen</th>
            </tr>
          </thead>
          <tbody>
            {#each households as row (row.user_id)}
              <tr>
                <td>{row.display_name || row.email}</td>
                <td>{row.session_count}</td>
                <td>{row.message_count}</td>
                <td>{row.summary_count}</td>
                <td>{row.fact_count}</td>
                <td class="muted">{row.last_seen ? formatFull(row.last_seen) : 'never'}</td>
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    </section>
  {/if}

  {#if status === 'ready'}
    <section class="section">
      <div class="section-header">
        <h2>Forget everything</h2>
      </div>
      <p class="intro muted">
        Deletes every message, every conversation summary and every fact above. Yours only, and it
        does not need an admin.
      </p>
      <div>
        <button class="btn danger" type="button" onclick={() => (confirmForget = true)}>
          Forget everything
        </button>
      </div>
    </section>
  {/if}
</div>

{#if deletingId}
  <DeleteConfirmModal
    title="Forget This"
    message={`Forget "${deletingLabel}"?`}
    confirmLabel={deleting ? 'Forgetting...' : 'Forget'}
    loading={deleting}
    onCancel={() => {
      deletingId = null;
      deletingLabel = '';
    }}
    onConfirm={handleDelete}
  />
{/if}

{#if confirmForget}
  <DeleteConfirmModal
    title="Forget Everything"
    message="Delete every chat message, conversation summary and remembered fact of yours? This cannot be undone."
    confirmLabel={forgetting ? 'Forgetting...' : 'Forget everything'}
    loading={forgetting}
    onCancel={() => (confirmForget = false)}
    onConfirm={handleForget}
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

  .section-header h2 {
    margin: 0;
    font-size: 18px;
  }

  .intro {
    margin: 0;
    max-width: 68ch;
    font-size: 13px;
  }

  .muted {
    color: var(--muted);
  }

  .error {
    color: var(--danger, #e5484d);
    font-size: 13px;
  }

  .card {
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 16px;
    padding: 16px;
  }

  .card.empty {
    padding: 24px 16px;
    text-align: center;
  }

  .list {
    display: grid;
    gap: 16px;
  }

  .group {
    display: grid;
    gap: 8px;
  }

  .group-label {
    font-size: 12px;
    letter-spacing: 0.08em;
    text-transform: uppercase;
    color: var(--muted);
  }

  .row {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 12px;
    padding: 12px 14px;
  }

  .row-main {
    display: grid;
    gap: 4px;
    min-width: 0;
    flex: 1;
  }

  .row-title {
    font-size: 14px;
  }

  .row-sub {
    font-size: 12px;
  }

  .summary {
    font-size: 13px;
    line-height: 1.5;
    max-width: 78ch;
  }

  .row-actions {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .edit-form {
    display: grid;
    gap: 8px;
    width: 100%;
  }

  .form {
    display: grid;
    gap: 10px;
  }

  .form h3 {
    margin: 0 0 4px;
    font-size: 16px;
  }

  .form label {
    font-size: 12px;
    color: var(--muted);
  }

  input[type='text'] {
    padding: 8px 10px;
    border-radius: 10px;
    border: 1px solid var(--ring);
    background: transparent;
    color: var(--ink);
    font-size: 13px;
    width: 100%;
  }

  .count {
    font-size: 12px;
  }

  .table-wrap {
    overflow-x: auto;
    border: 1px solid var(--ring);
    border-radius: 16px;
  }

  table {
    width: 100%;
    border-collapse: collapse;
    font-size: 13px;
  }

  th,
  td {
    text-align: left;
    padding: 10px 14px;
    white-space: nowrap;
  }

  th {
    font-size: 12px;
    font-weight: 500;
    color: var(--muted);
    border-bottom: 1px solid var(--ring);
  }

  tbody tr + tr td {
    border-top: 1px solid var(--ring);
  }
</style>
