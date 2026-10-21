<script lang="ts">
  import type { Snippet } from 'svelte';

  interface Props {
    label: string;
    headActions?: Snippet;
    children: Snippet;
  }

  let { label, headActions, children }: Props = $props();
</script>

<section class="detail-section">
  <div class="section-head">
    <div class="label">{label}</div>
    {@render headActions?.()}
  </div>
  {@render children()}
</section>

<style>
  .detail-section {
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 16px;
    padding: 24px;
  }

  .section-head {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    margin-bottom: 12px;
  }

  .label {
    font-size: 11px;
    font-weight: 800;
    color: var(--muted);
    text-transform: uppercase;
    letter-spacing: 0.1em;
  }

  /* Phone widths: trim the padding so season rows keep to one line. */
  @media (max-width: 600px) {
    .detail-section {
      padding: 16px 10px;
    }
  }
</style>
