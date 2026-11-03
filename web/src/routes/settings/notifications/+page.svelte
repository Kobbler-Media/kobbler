<script lang="ts">
  import { onMount } from 'svelte';
  import { showMessage } from '#lib/context/app.svelte.js';
  import { getAuthState } from '#lib/stores/auth.svelte.js';
  import { createWebPushController } from '#lib/services/webpush.svelte.js';
  import {
    listNotificationTypes,
    listNotificationAgents,
    createNotificationAgent,
    updateNotificationAgent,
    deleteNotificationAgent,
    testNotificationAgent,
    type NotificationAgentType,
    type NotificationAgent,
    type CreateNotificationAgentRequest,
    type UpdateNotificationAgentRequest
  } from '#lib/api/client.js';

  const auth = getAuthState();

  let loading = $state(true);
  let error = $state<string | null>(null);

  // Data
  let agentTypes = $state<NotificationAgentType[]>([]);
  let agents = $state<NotificationAgent[]>([]);

  // Add agent state
  let adding = $state(false);
  let selectedType = $state<NotificationAgentType | null>(null);
  let addName = $state('');
  let addConfig = $state<Record<string, string>>({});
  let addTriggers = $state<Record<string, boolean>>({});
  let saving = $state(false);

  // Edit agent state
  let editingId = $state<string | null>(null);
  let editName = $state('');
  let editConfig = $state<Record<string, string>>({});
  let editTriggers = $state<Record<string, boolean>>({});

  const webPush = createWebPushController();

  // Derived: the auto-managed webpush agent (hidden from generic agents list)
  let webPushAgent = $derived(agents.find((a) => a.agent_type === 'webpush'));
  let nonWebPushAgents = $derived(agents.filter((a) => a.agent_type !== 'webpush'));
  let addableTypes = $derived(agentTypes.filter((t) => t.id !== 'webpush'));
  let webPushTriggers = $derived(
    (agentTypes.find((t) => t.id === 'webpush')?.supported_triggers ?? []).filter(
      (t) => t !== 'on_test'
    )
  );

  onMount(async () => {
    await Promise.all([loadData(), webPush.refresh()]);
  });

  async function loadData() {
    loading = true;
    error = null;
    try {
      [agentTypes, agents] = await Promise.all([listNotificationTypes(), listNotificationAgents()]);
    } catch (e) {
      error = e instanceof Error ? e.message : 'Failed to load notification settings';
    } finally {
      loading = false;
    }
  }

  // ── Add Agent ──────────────────────────────────────────────────────────

  function startAdd() {
    adding = true;
    selectedType = null;
    addName = '';
    addConfig = {};
    addTriggers = {};
  }

  function selectType(type: NotificationAgentType) {
    selectedType = type;
    addName = '';
    addConfig = {};
    for (const field of type.config_fields) {
      addConfig[field.key] = field.default ?? '';
    }
    addTriggers = {};
    for (const trigger of type.supported_triggers) {
      addTriggers[trigger] = true;
    }
  }

  function cancelAdd() {
    adding = false;
    selectedType = null;
  }

  async function saveNewAgent() {
    if (!selectedType || !addName.trim()) return;
    saving = true;
    try {
      const triggers = Object.entries(addTriggers)
        .filter(([, v]) => v)
        .map(([k]) => k);
      const input: CreateNotificationAgentRequest = {
        agent_type: selectedType.id,
        name: addName.trim(),
        config: addConfig,
        triggers
      };
      await createNotificationAgent(input);
      adding = false;
      selectedType = null;
      showMessage('Agent created');
      await loadData();
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to create agent');
    } finally {
      saving = false;
    }
  }

  // ── Edit Agent ─────────────────────────────────────────────────────────

  function startEdit(agent: NotificationAgent) {
    editingId = agent.id;
    editName = agent.name;
    editConfig = {};
    const typeInfo = agentTypes.find((t) => t.id === agent.agent_type);
    if (typeInfo) {
      for (const field of typeInfo.config_fields) {
        const val = agent.config[field.key];
        editConfig[field.key] = typeof val === 'string' ? val : val != null ? String(val) : '';
      }
    }
    editTriggers = {};
    const typeObj = agentTypes.find((t) => t.id === agent.agent_type);
    if (typeObj) {
      for (const trigger of typeObj.supported_triggers) {
        editTriggers[trigger] = Array.isArray(agent.triggers) && agent.triggers.includes(trigger);
      }
    }
  }

  function cancelEdit() {
    editingId = null;
  }

  async function saveEdit() {
    if (!editingId || !editName.trim()) return;
    saving = true;
    try {
      const triggers = Object.entries(editTriggers)
        .filter(([, v]) => v)
        .map(([k]) => k);
      // Only send config fields that are non-empty (avoid overwriting secrets with empty strings)
      const config: Record<string, unknown> = {};
      for (const [key, val] of Object.entries(editConfig)) {
        if (val !== '' && val !== '\u2022\u2022\u2022\u2022\u2022\u2022') {
          config[key] = val;
        }
      }
      const input: UpdateNotificationAgentRequest = {
        name: editName.trim(),
        config,
        triggers
      };
      await updateNotificationAgent(editingId, input);
      editingId = null;
      showMessage('Agent updated');
      await loadData();
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to update agent');
    } finally {
      saving = false;
    }
  }

  // ── Toggle / Delete / Test ─────────────────────────────────────────────

  async function toggleAgent(agent: NotificationAgent) {
    try {
      await updateNotificationAgent(agent.id, { enabled: !agent.enabled });
      await loadData();
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to toggle agent');
    }
  }

  async function removeAgent(agent: NotificationAgent) {
    if (!confirm(`Delete "${agent.name}"? This cannot be undone.`)) return;
    try {
      await deleteNotificationAgent(agent.id);
      showMessage('Agent deleted');
      await loadData();
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to delete agent');
    }
  }

  async function testAgent(agent: NotificationAgent) {
    try {
      await testNotificationAgent(agent.id);
      showMessage('Test notification sent');
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to send test');
    }
  }

  // ── Web Push ───────────────────────────────────────────────────────────

  async function handleSubscribe() {
    if (await webPush.subscribe()) {
      await loadData(); // backend auto-creates the webpush agent on first subscribe
      showMessage('Subscribed to push notifications');
    }
  }

  async function handleUnsubscribe() {
    if (await webPush.unsubscribe()) {
      showMessage('Unsubscribed from push notifications');
    } else if (webPush.error) {
      showMessage(webPush.error);
    }
  }

  // ── Web Push Agent Controls ───────────────────────────────────────────

  async function toggleWebPushTrigger(trigger: string) {
    if (!webPushAgent) return;
    const current = Array.isArray(webPushAgent.triggers) ? webPushAgent.triggers : [];
    const triggers = current.includes(trigger)
      ? current.filter((t) => t !== trigger)
      : [...current, trigger];
    try {
      await updateNotificationAgent(webPushAgent.id, { triggers });
      await loadData();
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to update triggers');
    }
  }

  async function toggleWebPushEnabled() {
    if (!webPushAgent) return;
    try {
      await updateNotificationAgent(webPushAgent.id, { enabled: !webPushAgent.enabled });
      await loadData();
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to toggle web push');
    }
  }

  async function testWebPush() {
    if (!webPushAgent) return;
    try {
      await testNotificationAgent(webPushAgent.id);
      showMessage('Test notification sent');
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Failed to send test');
    }
  }

  // ── Helpers ────────────────────────────────────────────────────────────

  function triggerLabel(trigger: string): string {
    const labels: Record<string, string> = {
      on_grab: 'On Grab',
      on_import: 'On Import',
      on_failed: 'On Failed',
      on_media_corrupt: 'On Corrupt Media',
      on_test: 'Test'
    };
    return labels[trigger] ?? trigger;
  }

  function inputType(fieldType: string): string {
    switch (fieldType) {
      case 'secret':
        return 'password';
      case 'url':
        return 'url';
      case 'number':
        return 'number';
      default:
        return 'text';
    }
  }

  function getAgentType(agent: NotificationAgent): NotificationAgentType | undefined {
    return agentTypes.find((t) => t.id === agent.agent_type);
  }
</script>

<svelte:head>
  <title>Notification Settings — Kobbler</title>
</svelte:head>

{#if !auth.isAdmin}
  <div class="permission-denied">
    <h2>Access Denied</h2>
    <p>You don't have permission to view this page.</p>
  </div>
{:else}
  <div class="notifications-page">
    <!-- ── Notification Agents ────────────────────────────────────────── -->
    <div class="section-header">
      <h2>Notification Agents</h2>
      {#if !adding}
        <button class="btn small" type="button" onclick={startAdd}>Add Agent</button>
      {/if}
    </div>

    <p class="muted description">
      Configure how Kobbler sends notifications when events occur, such as downloads starting,
      imports completing, or failures.
    </p>

    {#if loading}
      <div class="loading-card">Loading notification settings...</div>
    {:else if error}
      <div class="error">{error}</div>
    {:else}
      <!-- Add Agent Form -->
      {#if adding}
        <div class="provider-card">
          <div class="provider-edit-form">
            {#if !selectedType}
              <div class="provider-edit-header">
                <div class="provider-logo">
                  <svg viewBox="0 0 24 24" width="24" height="24" fill="currentColor">
                    <path
                      d="M12 22c1.1 0 2-.9 2-2h-4a2 2 0 0 0 2 2zm6-6v-5c0-3.07-1.64-5.64-4.5-6.32V4c0-.83-.67-1.5-1.5-1.5s-1.5.67-1.5 1.5v.68C7.63 5.36 6 7.92 6 11v5l-2 2v1h16v-1l-2-2z"
                    ></path>
                  </svg>
                </div>
                <h4>Select Agent Type</h4>
              </div>

              <div class="type-selector">
                {#each addableTypes as type (type.id)}
                  <button class="type-option" type="button" onclick={() => selectType(type)}>
                    <div class="type-option-name">{type.display_name}</div>
                    <div class="type-option-desc muted">{type.description}</div>
                  </button>
                {/each}
              </div>

              <div class="provider-edit-actions">
                <button class="btn alt small" type="button" onclick={cancelAdd}>Cancel</button>
              </div>
            {:else}
              <div class="provider-edit-header">
                <div class="provider-logo">
                  <svg viewBox="0 0 24 24" width="24" height="24" fill="currentColor">
                    <path
                      d="M12 22c1.1 0 2-.9 2-2h-4a2 2 0 0 0 2 2zm6-6v-5c0-3.07-1.64-5.64-4.5-6.32V4c0-.83-.67-1.5-1.5-1.5s-1.5.67-1.5 1.5v.68C7.63 5.36 6 7.92 6 11v5l-2 2v1h16v-1l-2-2z"
                    ></path>
                  </svg>
                </div>
                <h4>New {selectedType.display_name} Agent</h4>
              </div>

              <div class="provider-edit-fields">
                <label>
                  Name
                  <input type="text" bind:value={addName} placeholder="e.g. My Phone" />
                </label>

                {#each selectedType.config_fields as field (field.key)}
                  <label>
                    {field.label}
                    {#if field.required}<span class="required">*</span>{/if}
                    <input
                      type={inputType(field.field_type)}
                      bind:value={addConfig[field.key]}
                      placeholder={field.default ?? ''}
                      required={field.required}
                    />
                    {#if field.description}
                      <span class="field-hint">
                        {field.description}
                        {#if field.help_url}
                          &mdash; <a href={field.help_url} target="_blank" rel="noopener"
                            >Learn more</a
                          >
                        {/if}
                      </span>
                    {:else if field.help_url}
                      <span class="field-hint">
                        <a href={field.help_url} target="_blank" rel="noopener">Learn more</a>
                      </span>
                    {/if}
                  </label>
                {/each}

                <fieldset class="trigger-fieldset">
                  <legend>Triggers</legend>
                  {#each selectedType.supported_triggers.filter((t) => t !== 'on_test') as trigger (trigger)}
                    <label class="checkbox-label">
                      <input type="checkbox" bind:checked={addTriggers[trigger]} />
                      {triggerLabel(trigger)}
                    </label>
                  {/each}
                </fieldset>
              </div>

              <div class="provider-edit-actions">
                <button
                  class="btn small"
                  type="button"
                  onclick={saveNewAgent}
                  disabled={saving || !addName.trim()}
                >
                  {saving ? 'Saving...' : 'Save'}
                </button>
                <button class="btn alt small" type="button" onclick={cancelAdd} disabled={saving}>
                  Cancel
                </button>
              </div>
            {/if}
          </div>
        </div>
      {/if}

      <!-- Agent Cards (webpush managed in dedicated section below) -->
      {#each nonWebPushAgents as agent (agent.id)}
        <div class="provider-card" class:disabled={!agent.enabled}>
          {#if editingId === agent.id}
            <!-- Edit Form -->
            {@const typeInfo = getAgentType(agent)}
            <div class="provider-edit-form">
              <div class="provider-edit-header">
                <div class="provider-logo">
                  <svg viewBox="0 0 24 24" width="24" height="24" fill="currentColor">
                    <path
                      d="M12 22c1.1 0 2-.9 2-2h-4a2 2 0 0 0 2 2zm6-6v-5c0-3.07-1.64-5.64-4.5-6.32V4c0-.83-.67-1.5-1.5-1.5s-1.5.67-1.5 1.5v.68C7.63 5.36 6 7.92 6 11v5l-2 2v1h16v-1l-2-2z"
                    ></path>
                  </svg>
                </div>
                <h4>Edit {agent.display_name}</h4>
              </div>

              <div class="provider-edit-fields">
                <label>
                  Name
                  <input type="text" bind:value={editName} placeholder="Agent name" />
                </label>

                {#if typeInfo}
                  {#each typeInfo.config_fields as field (field.key)}
                    <label>
                      {field.label}
                      {#if field.required}<span class="required">*</span>{/if}
                      <input
                        type={inputType(field.field_type)}
                        bind:value={editConfig[field.key]}
                        placeholder={field.field_type === 'secret'
                          ? '(unchanged)'
                          : (field.default ?? '')}
                      />
                      {#if field.description}
                        <span class="field-hint">
                          {field.description}
                          {#if field.help_url}
                            &mdash; <a href={field.help_url} target="_blank" rel="noopener"
                              >Learn more</a
                            >
                          {/if}
                        </span>
                      {:else if field.help_url}
                        <span class="field-hint">
                          <a href={field.help_url} target="_blank" rel="noopener">Learn more</a>
                        </span>
                      {/if}
                    </label>
                  {/each}

                  <fieldset class="trigger-fieldset">
                    <legend>Triggers</legend>
                    {#each typeInfo.supported_triggers.filter((t) => t !== 'on_test') as trigger (trigger)}
                      <label class="checkbox-label">
                        <input type="checkbox" bind:checked={editTriggers[trigger]} />
                        {triggerLabel(trigger)}
                      </label>
                    {/each}
                  </fieldset>
                {/if}
              </div>

              <div class="provider-edit-actions">
                <button
                  class="btn small"
                  type="button"
                  onclick={saveEdit}
                  disabled={saving || !editName.trim()}
                >
                  {saving ? 'Saving...' : 'Save'}
                </button>
                <button class="btn alt small" type="button" onclick={cancelEdit} disabled={saving}>
                  Cancel
                </button>
              </div>
            </div>
          {:else}
            <!-- Display Card -->
            <div class="provider-row">
              <div class="provider-info">
                <div class="provider-header">
                  <div class="provider-logo">
                    <svg viewBox="0 0 24 24" width="24" height="24" fill="currentColor">
                      <path
                        d="M12 22c1.1 0 2-.9 2-2h-4a2 2 0 0 0 2 2zm6-6v-5c0-3.07-1.64-5.64-4.5-6.32V4c0-.83-.67-1.5-1.5-1.5s-1.5.67-1.5 1.5v.68C7.63 5.36 6 7.92 6 11v5l-2 2v1h16v-1l-2-2z"
                      ></path>
                    </svg>
                  </div>
                  <span class="provider-name">{agent.name}</span>
                  <span class="config-badge">{agent.display_name}</span>
                  <span class="status-badge" class:enabled={agent.enabled}>
                    {agent.enabled ? 'Enabled' : 'Disabled'}
                  </span>
                </div>

                <p class="provider-description muted">{agent.description}</p>

                <div class="provider-capabilities">
                  {#each (Array.isArray(agent.triggers) ? agent.triggers : []).filter((t) => t !== 'on_test') as trigger (trigger)}
                    <span class="capability-badge">{triggerLabel(trigger)}</span>
                  {/each}
                </div>
              </div>

              <div class="provider-actions">
                <button class="btn alt small" type="button" onclick={() => toggleAgent(agent)}>
                  {agent.enabled ? 'Disable' : 'Enable'}
                </button>
                <button class="btn alt small" type="button" onclick={() => startEdit(agent)}>
                  Edit
                </button>
                <button class="btn alt small" type="button" onclick={() => testAgent(agent)}>
                  Test
                </button>
                <button
                  class="btn alt small danger"
                  type="button"
                  onclick={() => removeAgent(agent)}
                >
                  Delete
                </button>
              </div>
            </div>
          {/if}
        </div>
      {/each}

      {#if nonWebPushAgents.length === 0 && !adding}
        <div class="sync-info muted">
          <svg viewBox="0 0 24 24" width="18" height="18" fill="currentColor">
            <path
              d="M12 2C6.48 2 2 6.48 2 12s4.48 10 10 10 10-4.48 10-10S17.52 2 12 2zm1 15h-2v-2h2v2zm0-4h-2V7h2v6z"
            ></path>
          </svg>
          <span>No notification agents configured. Add one to get started.</span>
        </div>
      {/if}
    {/if}

    <!-- ── Web Push Notifications ─────────────────────────────────────── -->
    <div class="section-header section-gap">
      <h2>Web Push Notifications</h2>
    </div>

    <p class="muted description">
      Receive push notifications directly in your browser. Your browser must grant notification
      permission for this to work.
    </p>

    {#if !webPush.supported}
      <div class="sync-info muted">
        <svg viewBox="0 0 24 24" width="18" height="18" fill="currentColor">
          <path
            d="M12 2C6.48 2 2 6.48 2 12s4.48 10 10 10 10-4.48 10-10S17.52 2 12 2zm1 15h-2v-2h2v2zm0-4h-2V7h2v6z"
          ></path>
        </svg>
        <span>Web push notifications are not supported in this browser.</span>
      </div>
    {:else}
      <div class="provider-card" class:disabled={webPushAgent && !webPushAgent.enabled}>
        <div class="provider-row">
          <div class="provider-info">
            <div class="provider-header">
              <div class="provider-logo">
                <svg viewBox="0 0 24 24" width="24" height="24" fill="currentColor">
                  <path
                    d="M12 22c1.1 0 2-.9 2-2h-4a2 2 0 0 0 2 2zm6-6v-5c0-3.07-1.64-5.64-4.5-6.32V4c0-.83-.67-1.5-1.5-1.5s-1.5.67-1.5 1.5v.68C7.63 5.36 6 7.92 6 11v5l-2 2v1h16v-1l-2-2z"
                  ></path>
                </svg>
              </div>
              <span class="provider-name">Browser Push</span>
              <span class="status-badge" class:enabled={webPush.subscribed}>
                {webPush.subscribed ? 'Subscribed' : 'Not Subscribed'}
              </span>
              {#if webPush.subscribed && webPushAgent}
                <span class="status-badge" class:enabled={webPushAgent.enabled}>
                  {webPushAgent.enabled ? 'Enabled' : 'Disabled'}
                </span>
              {/if}
            </div>
            <p class="provider-description muted">
              Receive notifications in this browser when events occur. Browser notification
              permission is required.
            </p>

            {#if webPush.error}
              <div class="error" style="text-align: left; font-size: 12px; padding: 8px 12px;">
                {webPush.error}
              </div>
            {/if}

            {#if webPush.subscribed && webPushAgent}
              <fieldset class="trigger-fieldset">
                <legend>Notify on</legend>
                {#each webPushTriggers as trigger (trigger)}
                  <label class="checkbox-label">
                    <input
                      type="checkbox"
                      checked={Array.isArray(webPushAgent.triggers) &&
                        webPushAgent.triggers.includes(trigger)}
                      onchange={() => toggleWebPushTrigger(trigger)}
                    />
                    {triggerLabel(trigger)}
                  </label>
                {/each}
              </fieldset>
            {/if}
          </div>

          <div class="provider-actions">
            {#if webPush.subscribed}
              {#if webPushAgent}
                <button class="btn alt small" type="button" onclick={toggleWebPushEnabled}>
                  {webPushAgent.enabled ? 'Disable' : 'Enable'}
                </button>
                <button class="btn alt small" type="button" onclick={testWebPush}>Test</button>
              {/if}
              <button
                class="btn alt small danger"
                type="button"
                onclick={handleUnsubscribe}
                disabled={webPush.loading}
              >
                {webPush.loading ? 'Working...' : 'Unsubscribe'}
              </button>
            {:else}
              <button
                class="btn small"
                type="button"
                onclick={handleSubscribe}
                disabled={webPush.loading}
              >
                {webPush.loading ? 'Working...' : 'Subscribe'}
              </button>
            {/if}
          </div>
        </div>
      </div>
    {/if}
  </div>
{/if}

<style>
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

  .notifications-page {
    display: grid;
    gap: 16px;
  }

  .section-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
  }

  .section-header h2 {
    margin: 0;
    font-size: 18px;
  }

  .section-gap {
    margin-top: 24px;
  }

  .description {
    margin: 0;
    font-size: 13px;
    line-height: 1.5;
  }

  /* Provider cards */
  .provider-card {
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 14px;
    padding: 16px;
    transition: opacity 0.15s;
  }

  .provider-card.disabled {
    opacity: 0.7;
  }

  .provider-row {
    display: flex;
    justify-content: space-between;
    gap: 16px;
  }

  .provider-info {
    flex: 1;
    display: grid;
    gap: 8px;
  }

  .provider-header {
    display: flex;
    align-items: center;
    gap: 12px;
    flex-wrap: wrap;
  }

  .provider-logo {
    width: 32px;
    height: 32px;
    background: var(--ring);
    border-radius: 8px;
    display: flex;
    align-items: center;
    justify-content: center;
    color: var(--accent);
    flex-shrink: 0;
  }

  .provider-name {
    font-weight: 600;
    font-size: 15px;
  }

  .status-badge {
    font-size: 11px;
    padding: 2px 8px;
    border-radius: 6px;
    background: var(--ring);
    color: var(--muted);
  }

  .status-badge.enabled {
    background: var(--accent);
    color: white;
  }

  .config-badge {
    font-size: 11px;
    padding: 2px 8px;
    border-radius: 6px;
    background: #22c55e;
    color: white;
  }

  .provider-capabilities {
    display: flex;
    flex-wrap: wrap;
    gap: 6px;
  }

  .capability-badge {
    font-size: 10px;
    padding: 2px 6px;
    border-radius: 4px;
    background: var(--ring);
    color: var(--muted);
  }

  .provider-description {
    font-size: 13px;
    margin: 0;
  }

  .provider-actions {
    display: flex;
    flex-direction: column;
    gap: 6px;
    flex-shrink: 0;
  }

  /* Edit form */
  .provider-edit-form {
    display: grid;
    gap: 16px;
  }

  .provider-edit-header {
    display: flex;
    align-items: center;
    gap: 12px;
  }

  .provider-edit-header h4 {
    margin: 0;
    font-size: 16px;
  }

  .provider-edit-fields {
    display: grid;
    gap: 12px;
  }

  .provider-edit-fields label {
    display: grid;
    gap: 6px;
    font-size: 12px;
    color: var(--muted);
  }

  .provider-edit-fields input[type='text'],
  .provider-edit-fields input[type='password'],
  .provider-edit-fields input[type='url'],
  .provider-edit-fields input[type='number'] {
    padding: 10px 12px;
    border-radius: 10px;
    border: 1px solid var(--ring);
    background: transparent;
    color: var(--ink);
    font-size: 13px;
  }

  .provider-edit-actions {
    display: flex;
    gap: 8px;
  }

  .required {
    color: var(--danger, #ef4444);
    margin-left: 2px;
  }

  .trigger-fieldset {
    border: 1px solid var(--ring);
    border-radius: 10px;
    padding: 12px;
    margin: 0;
    display: flex;
    flex-wrap: wrap;
    gap: 8px 16px;
  }

  .trigger-fieldset legend {
    font-size: 12px;
    color: var(--muted);
    padding: 0 6px;
  }

  .checkbox-label {
    display: flex !important;
    flex-direction: row !important;
    align-items: center;
    gap: 8px !important;
    cursor: pointer;
    color: var(--ink);
    font-size: 13px !important;
  }

  .checkbox-label input[type='checkbox'] {
    width: 16px;
    height: 16px;
    cursor: pointer;
  }

  .field-hint {
    font-size: 11px;
    color: var(--muted);
  }

  .field-hint a {
    color: var(--accent);
  }

  /* Type selector */
  .type-selector {
    display: grid;
    gap: 8px;
  }

  .type-option {
    display: grid;
    gap: 4px;
    padding: 12px;
    background: transparent;
    border: 1px solid var(--ring);
    border-radius: 10px;
    text-align: left;
    cursor: pointer;
    color: var(--ink);
    transition: border-color 0.15s;
  }

  .type-option:hover {
    border-color: var(--accent);
  }

  .type-option-name {
    font-weight: 600;
    font-size: 14px;
  }

  .type-option-desc {
    font-size: 12px;
  }

  /* Buttons */
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

  .btn.danger {
    color: var(--danger, #ef4444);
  }

  /* Status messages */
  .sync-info {
    display: flex;
    align-items: center;
    gap: 8px;
    padding: 12px 16px;
    background: var(--ring);
    border: 1px solid var(--ring);
    border-radius: 10px;
    font-size: 13px;
    color: var(--muted);
  }

  /* Utilities */
  .loading-card {
    padding: 24px;
    text-align: center;
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 16px;
    color: var(--muted);
  }

  .error {
    padding: 16px;
    text-align: center;
    border-radius: 12px;
    border: 1px solid var(--danger);
    background: var(--card);
    color: var(--danger);
  }

  @media (max-width: 768px) {
    .provider-row {
      flex-direction: column;
    }

    .provider-actions {
      flex-direction: row;
      flex-wrap: wrap;
    }
  }
</style>
