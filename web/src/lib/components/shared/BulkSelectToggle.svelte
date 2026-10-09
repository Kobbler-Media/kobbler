<script lang="ts">
  interface Props {
    active: boolean;
    title: string;
    onToggle: () => void;
  }

  let { active, title, onToggle }: Props = $props();
</script>

<button
  class="select-toggle"
  class:active
  type="button"
  title={active ? 'Exit selection' : title}
  onclick={onToggle}
>
  <span class="material-icons-outlined">{active ? 'close' : 'checklist'}</span>
  {active ? 'Done' : 'Select'}
</button>

<style>
  .select-toggle {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    padding: 4px 10px;
    border-radius: 8px;
    border: 1px solid var(--ring);
    background: var(--bg);
    color: var(--muted);
    font-size: 12px;
    font-weight: 600;
    cursor: pointer;
    transition:
      background 0.15s,
      color 0.15s,
      border-color 0.15s;
  }

  .select-toggle .material-icons-outlined {
    font-size: 15px;
  }

  .select-toggle:hover {
    background: var(--bg-2);
    color: var(--ink);
  }

  .select-toggle.active {
    background: var(--accent);
    border-color: var(--accent);
    color: white;
  }
</style>
