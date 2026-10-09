<script lang="ts">
  interface Props {
    active: 'guide' | 'channels' | 'recordings' | 'scheduled';
  }

  let { active }: Props = $props();

  const tabs = [
    { id: 'guide', label: 'Guide', href: '/livetv' },
    { id: 'channels', label: 'Channels', href: '/livetv/channels' },
    { id: 'recordings', label: 'Recordings', href: '/livetv/recordings' },
    { id: 'scheduled', label: 'Scheduled', href: '/livetv/scheduled' }
  ] as const;
</script>

<nav class="tabs">
  {#each tabs as tab (tab.id)}
    <a href={tab.href} class="tab" class:active={tab.id === active}>{tab.label}</a>
  {/each}
</nav>

<style>
  .tabs {
    display: flex;
    gap: 24px;
    border-bottom: 1px solid var(--ring);
    margin-bottom: 14px;
  }

  .tab {
    padding: 0 1px 10px;
    font-size: 13px;
    font-weight: 500;
    color: var(--muted);
    text-decoration: none;
    box-shadow: inset 0 -2px 0 transparent;
  }

  .tab:hover {
    color: var(--ink);
  }

  .tab.active {
    font-weight: 600;
    color: var(--ink);
    box-shadow: inset 0 -2px 0 var(--accent);
  }
</style>
