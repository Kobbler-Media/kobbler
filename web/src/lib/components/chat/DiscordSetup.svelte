<script lang="ts">
  import { onMount } from 'svelte';
  import { copyText } from '#lib/context/app.svelte.js';
  import { listNotificationAgents, type NotificationAgent } from '#lib/api/client.js';

  const PORTAL_URL = 'https://discord.com/developers/applications';
  const INVITE_PERMISSIONS = 'View Channels, Send Messages, Read Message History, Embed Links';
  // View Channels + Send Messages + Embed Links + Read Message History.
  const INVITE_PERMISSION_BITS = '84992';

  let agent = $state<NotificationAgent | null>(null);
  let loading = $state(true);
  let error = $state('');

  const channelId = $derived.by(() => {
    const raw = agent?.config?.['channel_id'];
    return typeof raw === 'string' ? raw.trim() : '';
  });

  onMount(async () => {
    try {
      const agents = await listNotificationAgents();
      agent = agents.find((a) => a.agent_type === 'discord') ?? null;
    } catch (e) {
      error = e instanceof Error ? e.message : 'Could not read the Discord setup';
    } finally {
      loading = false;
    }
  });
</script>

<section class="section">
  <div class="section-header">
    <h2>Discord bot setup</h2>
    <span class="muted">No public URL, no open port</span>
  </div>

  <div class="card">
    {#if loading}
      <div class="muted">Reading the current setup…</div>
    {:else if error}
      <div class="error">{error}</div>
    {:else}
      <div class="fields">
        <div class="field">
          <div class="field-label">Bot token</div>
          {#if !agent}
            <div class="notice">
              <strong>No Discord bot configured.</strong> Start there: add a Discord agent under
              <a href="/settings/notifications">Settings → Automation → Notifications</a>. The same
              entry holds the bot token the assistant connects with and the channel notifications go
              to.
            </div>
          {:else}
            <p class="hint">
              Held by the “{agent.name}” agent under
              <a href="/settings/notifications">Settings → Automation → Notifications</a>. Kobbler
              opens the connection to Discord, so unlike Google Chat nothing here needs to be
              reachable from outside your network. Changing the token takes effect within a minute —
              no restart.
            </p>
          {/if}
        </div>

        <div class="field">
          <div class="field-label">Announcements channel</div>
          {#if channelId}
            <div class="value-row">
              <code class="value">{channelId}</code>
              <button
                class="btn alt small"
                type="button"
                onclick={() => copyText(channelId, 'Channel ID')}
              >
                Copy
              </button>
            </div>
            <p class="hint">
              Grabs, imports and failures are posted here. Questions asked in a DM are answered in
              that DM, not in this channel.
            </p>
          {:else}
            <p class="hint">
              Optional. Set a channel ID on the Discord agent to have new downloads announced
              somewhere everyone can see. Turn on <strong>Developer Mode</strong> in Discord
              (Settings → Advanced), then right-click the channel and
              <strong>Copy Channel ID</strong>.
            </p>
          {/if}
        </div>
      </div>

      <div class="steps">
        <div class="field-label">In the Discord developer portal</div>
        <ol>
          <li>
            Open <a href={PORTAL_URL} target="_blank" rel="noreferrer">Applications</a>, create one,
            then under <strong>Bot</strong> press <strong>Reset Token</strong> and copy it into the Discord
            agent.
          </li>
          <li>
            Invite the bot to the server <em>before</em> the next step — see the OAuth2 step below. Once
            it has joined it stays joined, and making the app private afterwards cannot strand you outside
            your own bot.
          </li>
          <li>
            Set <strong>Installation → Install Link</strong> to <strong>None</strong>, then turn
            <strong>Public Bot</strong> <strong>off</strong> on the Bot tab. In that order: with an
            install link still set, Discord refuses with
            <em>“Cannot have install fields on a private application.”</em>, printed at the top of
            the page rather than next to the toggle, so it reads as though it is about something
            else. Private stops anyone else adding the bot to their server and does not affect who
            can message it — DMs are gated by sharing a server, not by this setting.
          </li>
          <li>
            Leave <strong>Requires OAuth2 Code Grant</strong> <strong>off</strong>. With it on the
            invite link fails with an error that does not mention this setting.
          </li>
          <li>
            Leave <strong>Interactions Endpoint URL</strong> blank. Setting it sends button taps to a
            web address instead of to kobbler, and the buttons stop working.
          </li>
          <li>
            Leave all three <strong>Privileged Gateway Intents</strong> off. Discord always delivers message
            text in DMs with the bot and in messages that @-mention it, and those are the only ones it
            answers.
          </li>
          <li>
            Under <strong>OAuth2 → URL Generator</strong>, tick <code>bot</code> — not
            <code>applications.commands</code>, there are no slash commands — then
            <em>{INVITE_PERMISSIONS}</em> (permissions <code>{INVITE_PERMISSION_BITS}</code>). Open
            the generated link and add the bot to a server you and whoever else uses this are both
            in.
          </li>
        </ol>
        <p class="hint">
          Embed Links is what lets a citation render as a tappable title rather than raw
          <code>[title](url)</code> markup. It only matters in a server channel — a DM has no channel
          permissions, so citations always render there.
        </p>
        <p class="hint">
          Discord only lets someone DM a bot they share a server with, so that last step is what
          makes DMs possible — a private server with just the two of you is enough. Each person then
          messages the bot directly, and their Discord account gets bound to a kobbler user the
          first time they do.
        </p>
        <p class="hint">
          To check it connected, look at the server's member list: Discord shows the bot as online
          only while kobbler is holding the connection open.
        </p>
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
