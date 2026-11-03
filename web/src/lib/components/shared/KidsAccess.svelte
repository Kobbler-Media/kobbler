<script lang="ts">
  import * as api from '#lib/api/client.js';
  import { showMessage } from '#lib/context/app.svelte.js';

  interface Props {
    kind: 'movie' | 'tv';
    id: string;
    override: boolean | null | undefined;
    allowed: boolean | undefined;
  }

  let { kind, id, override, allowed }: Props = $props();

  let current = $state<boolean | null>(null);
  let effective = $state(false);
  let saving = $state(false);

  $effect(() => {
    current = override ?? null;
    effective = allowed ?? false;
  });

  const options: { value: boolean | null; label: string }[] = [
    { value: null, label: 'Auto' },
    { value: true, label: 'Allow' },
    { value: false, label: 'Block' }
  ];

  // The rule lives on the server; after a change, ask it what Auto now resolves to.
  async function choose(value: boolean | null) {
    if (value === current || saving) return;
    saving = true;
    try {
      if (kind === 'movie') {
        await api.updateMovie(id, { kids_override: value });
        effective = (await api.getMovie(id)).kids_allowed ?? false;
      } else {
        await api.updateTvShow(id, { kids_override: value });
        effective = (await api.getTvShow(id)).kids_allowed ?? false;
      }
      current = value;
    } catch (e) {
      showMessage(e instanceof Error ? e.message : 'Could not update Kids access');
    } finally {
      saving = false;
    }
  }
</script>

<div class="kids-access" role="group" aria-label="Kids profiles">
  <span class="label">Kids</span>
  <div class="segments">
    {#each options as option (option.label)}
      <button
        type="button"
        class="segment"
        class:active={current === option.value}
        disabled={saving}
        aria-pressed={current === option.value}
        onclick={() => choose(option.value)}
      >
        {option.label}
      </button>
    {/each}
  </div>
  <span class="verdict" class:allowed={effective}>
    {effective ? 'Shown on Kids profiles' : 'Hidden from Kids profiles'}
  </span>
</div>

<style>
  .kids-access {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 8px;
    font-size: 12px;
  }

  .label {
    color: var(--muted);
    font-weight: 600;
    text-transform: uppercase;
    letter-spacing: 0.04em;
  }

  .segments {
    display: inline-flex;
    border: 1px solid var(--ring);
    border-radius: 999px;
    overflow: hidden;
  }

  .segment {
    padding: 4px 12px;
    border: none;
    background: transparent;
    color: var(--ink);
    font: inherit;
    cursor: pointer;
  }

  .segment + .segment {
    border-left: 1px solid var(--ring);
  }

  .segment.active {
    background: var(--accent);
    color: white;
  }

  .segment:disabled {
    cursor: progress;
  }

  .verdict {
    color: var(--muted);
  }

  .verdict.allowed {
    color: var(--accent-2, #22c55e);
  }
</style>
