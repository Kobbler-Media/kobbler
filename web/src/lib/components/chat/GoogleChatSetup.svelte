<script lang="ts">
  import { onMount } from 'svelte';
  import { copyText } from '#lib/context/app.svelte.js';
  import {
    getSystemConfig,
    listNotificationAgents,
    type NotificationAgent,
    type SystemConfig
  } from '#lib/api/client.js';

  const WEBHOOK_PATH = '/api/v1/chat/google_chat/webhook';
  const CONSOLE_CONFIG_URL =
    'https://console.cloud.google.com/apis/api/chat.googleapis.com/hangouts-chat';
  const CONSOLE_DASHBOARD_URL = 'https://console.cloud.google.com/home/dashboard';

  let systemConfig = $state<SystemConfig | null>(null);
  let agent = $state<NotificationAgent | null>(null);
  let loading = $state(true);
  let error = $state('');

  const publicUrl = $derived((systemConfig?.public_url ?? '').replace(/\/+$/, ''));
  const webhookUrl = $derived(publicUrl ? `${publicUrl}${WEBHOOK_PATH}` : '');
  const projectNumber = $derived.by(() => {
    const raw = agent?.config?.['project_number'];
    return typeof raw === 'string' ? raw.trim() : '';
  });

  onMount(async () => {
    try {
      const [config, agents] = await Promise.all([getSystemConfig(), listNotificationAgents()]);
      systemConfig = config;
      agent = agents.find((a) => a.agent_type === 'google_chat') ?? null;
    } catch (e) {
      error = e instanceof Error ? e.message : 'Could not read the Google Chat setup';
    } finally {
      loading = false;
    }
  });
</script>

<section class="section">
  <div class="section-header">
    <h2>Google Chat app setup</h2>
    <span class="muted">The two values Google asks you for</span>
  </div>

  <div class="card">
    {#if loading}
      <div class="muted">Reading the current setup…</div>
    {:else if error}
      <div class="error">{error}</div>
    {:else}
      <div class="fields">
        <div class="field">
          <div class="field-label">Webhook URL</div>
          {#if webhookUrl}
            <div class="value-row">
              <code class="value">{webhookUrl}</code>
              <button
                class="btn alt small"
                type="button"
                onclick={() => copyText(webhookUrl, 'Webhook URL')}
              >
                Copy
              </button>
            </div>
            <p class="hint">
              Goes in <strong>Connection settings → App URL</strong>. Google posts only to a public
              HTTPS address with a valid certificate, so this has to be reachable from outside your
              network.
            </p>
          {:else}
            <div class="notice">
              <strong>No public URL set.</strong> Kobbler can't work out its own address, so there
              is nothing to paste into Google yet. Set the address people reach this kobbler on
              under <a href="/settings/general">Settings → Core → General</a>, then come back here
              for the full URL.
            </div>
          {/if}
        </div>

        <div class="field">
          <div class="field-label">Audience (project number)</div>
          {#if !agent}
            <div class="notice">
              <strong>No Google Chat app configured.</strong> Start there: add a Google Chat agent
              under <a href="/settings/notifications">Settings → Automation → Notifications</a>. The
              same entry holds the service-account key kobbler posts with and the project number it
              checks inbound messages against.
            </div>
          {:else if projectNumber}
            <div class="value-row">
              <code class="value">{projectNumber}</code>
              <span class="muted small">from “{agent.name}”</span>
            </div>
            <p class="hint">
              Google signs every inbound request with this as the JWT audience. A wrong value
              rejects all of them. It is the project <em>number</em> from the Cloud project dashboard,
              not the project ID.
            </p>
          {:else}
            <div class="notice">
              <strong>Project number not set.</strong> Without it kobbler can't verify Google's
              signature and turns every message away. Take the project
              <em>number</em>
              from the
              <a href={CONSOLE_DASHBOARD_URL} target="_blank" rel="noreferrer">
                Cloud project dashboard
              </a>
              and put it in the “{agent.name}” agent's Project Number field under
              <a href="/settings/notifications">Settings → Automation → Notifications</a>.
            </div>
          {/if}
        </div>
      </div>

      <div class="steps">
        <div class="field-label">In the Google Cloud console</div>
        <ol>
          <li>
            Open
            <a href={CONSOLE_CONFIG_URL} target="_blank" rel="noreferrer">
              Google Chat API → Configuration
            </a>
            and fill in the app name, avatar URL and description. Until that page has been saved every
            call fails with <code>PERMISSION_DENIED</code>, however good the credential is.
          </li>
          <li>Under <strong>Connection settings</strong>, pick App URL and paste the URL above.</li>
          <li>
            Under <strong>Triggers</strong>, switch on <strong>Message</strong> and
            <strong>Added to space</strong>.
          </li>
          <li>
            Under <strong>Visibility</strong>, add the people or domain who should be able to add
            the app, then save and add it to a space.
          </li>
        </ol>
        <p class="hint">Changes take a couple of minutes to reach Chat.</p>
      </div>
    {/if}
  </div>
</section>

<style>
  .section {
    display: grid;
    gap: 16px;
  }

  .section-header {
    display: flex;
    flex-wrap: wrap;
    align-items: baseline;
    justify-content: space-between;
    gap: 16px;
  }

  .section-header h2 {
    margin: 0;
    font-size: 18px;
  }

  .card {
    display: grid;
    gap: 20px;
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 16px;
    padding: 16px;
  }

  .fields {
    display: grid;
    gap: 16px;
  }

  .field {
    display: grid;
    gap: 8px;
  }

  .field-label {
    font-size: 12px;
    color: var(--muted);
  }

  .value-row {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 12px;
  }

  .value {
    flex: 1;
    min-width: 240px;
    overflow-x: auto;
    white-space: nowrap;
    font-family: var(--mono, ui-monospace, monospace);
    font-size: 13px;
    padding: 8px 10px;
    border-radius: 10px;
    border: 1px solid var(--ring);
    color: var(--ink);
  }

  .hint,
  .notice {
    margin: 0;
    max-width: 74ch;
    font-size: 13px;
    line-height: 1.5;
  }

  .hint {
    color: var(--muted);
  }

  .notice {
    padding: 12px 14px;
    border-radius: 12px;
    border: 1px solid var(--warning, #f59e0b);
    color: var(--ink);
  }

  .notice strong {
    color: var(--warning, #f59e0b);
  }

  .steps {
    display: grid;
    gap: 8px;
    border-top: 1px solid var(--ring);
    padding-top: 16px;
  }

  .steps ol {
    margin: 0;
    padding-left: 20px;
    display: grid;
    gap: 8px;
    max-width: 74ch;
    font-size: 13px;
    line-height: 1.5;
  }

  .steps code {
    font-family: var(--mono, ui-monospace, monospace);
    font-size: 12px;
  }

  .muted {
    color: var(--muted);
  }

  .muted.small {
    font-size: 12px;
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

  .btn.alt {
    background: var(--card);
    border: 1px solid var(--ring);
    color: var(--ink);
  }

  .btn.small {
    padding: 6px 10px;
    font-size: 12px;
  }

  .error {
    padding: 16px;
    text-align: center;
    border-radius: 12px;
    border: 1px solid var(--danger);
    color: var(--danger);
  }

  a {
    color: var(--accent);
  }
</style>
