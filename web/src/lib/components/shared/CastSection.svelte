<script lang="ts" module>
  export interface CastEntry {
    key: string;
    name: string;
    role: string | null;
    imageUrl: string | null;
    onSelect?: () => void;
  }
</script>

<script lang="ts">
  import DetailSection from './DetailSection.svelte';
  import PersonCard from './PersonCard.svelte';

  interface Props {
    members: CastEntry[];
    limit?: number;
  }

  let { members, limit = 12 }: Props = $props();
</script>

{#if members.length > 0}
  <DetailSection label="Cast">
    <div class="cast-grid">
      {#each members.slice(0, limit) as member (member.key)}
        {#if member.onSelect}
          <button type="button" class="cast-btn" onclick={member.onSelect}>
            <PersonCard name={member.name} role={member.role} imageUrl={member.imageUrl} />
          </button>
        {:else}
          <PersonCard name={member.name} role={member.role} imageUrl={member.imageUrl} />
        {/if}
      {/each}
    </div>
  </DetailSection>
{/if}

<style>
  .cast-grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(180px, 1fr));
    gap: 12px;
  }

  .cast-btn {
    display: block;
    width: 100%;
    padding: 0;
    border: none;
    background: transparent;
    cursor: pointer;
    text-align: left;
    border-radius: 12px;
    transition: all 0.2s;
  }

  .cast-btn:hover {
    background: var(--bg-2);
    transform: translateY(-2px);
  }
</style>
