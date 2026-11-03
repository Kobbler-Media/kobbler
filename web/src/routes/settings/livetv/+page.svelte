<script lang="ts">
  import { onMount } from 'svelte';
  import { formatAge } from '@koblibs/ui';
  import ErrorBanner from '#lib/components/shared/ErrorBanner.svelte';
  import TunerMeter from '#lib/components/livetv/TunerMeter.svelte';
  import OptionRow from '#lib/components/livetv/OptionRow.svelte';
  import ToggleRow from '#lib/components/livetv/ToggleRow.svelte';
  import { showMessage } from '#lib/context/app.svelte.js';
  import { getLivetvState, fetchLivetvTuners, setLivetvTuners } from '#lib/stores/livetv.svelte.js';
  import * as api from '#lib/api/client.js';
  import type { SystemConfig, SystemConfigUpdate } from '#lib/api/client.js';
  import MetaLine from '#lib/components/shared/MetaLine.svelte';
  import type { LivetvDiscoveredTuner } from '#lib/types.js';

  const livetv = getLivetvState();

  let config = $state<SystemConfig | null>(null);
  let loading = $state(true);
  let error = $state('');
  let savingKey = $state<string | null>(null);

  let discovering = $state(false);
  let candidates = $state<LivetvDiscoveredTuner[]>([]);
  let manualUrl = $state('');
  let adding = $state(false);
  let scanning = $state(false);

  const AD_GENRES = ['News', 'Sports'];

  const guideSource = $derived(config?.livetv_guide_source ?? 'tuner');
  const bufferPolicy = $derived(config?.livetv_buffer_policy ?? 'tune_in');
  const bufferCap = $derived(config?.livetv_buffer_cap_hours ?? 3);
  const clashPolicy = $derived(config?.livetv_clash_policy ?? 'keep_playing');
  const adPosture = $derived(config?.livetv_ad_posture ?? 'shade');
  const adDisableGenres = $derived(config?.livetv_ad_disable_genres ?? AD_GENRES);
  const adTrain = $derived(config?.livetv_ad_train_from_corrections ?? true);
  const deinterlace = $derived(config?.livetv_deinterlace_web ?? true);
  const transcodeHeight = $derived(config?.livetv_transcode_height ?? 720);
  const recordingPath = $derived(config?.livetv_recording_path ?? '');

  const tuner = $derived(livetv.primary);

  const tunerLine = $derived.by(() => {
    if (!tuner) return ['No tuner registered.'];
    const parts = [tuner.friendly_name || tuner.model_number, tuner.device_id];
    try {
      parts.push(new URL(tuner.base_url).hostname);
    } catch {
      parts.push(tuner.base_url);
    }
    if (tuner.firmware_version) parts.push(`firmware ${tuner.firmware_version}`);
    return parts;
  });

  const scanLine = $derived.by(() => {
    if (!tuner) return [];
    const parts = [
      `${tuner.channel_count} service${tuner.channel_count === 1 ? '' : 's'}`,
      `${tuner.enabled_channel_count} enabled`
    ];
    if (tuner.last_scan_at) parts.push(`scanned ${formatAge(tuner.last_scan_at)}`);
    if (tuner.last_guide_sync_at) parts.push(`guide synced ${formatAge(tuner.last_guide_sync_at)}`);
    return [...parts, 'one tuner per stream, even on the same multiplex.'];
  });

  async function loadConfig() {
    loading = true;
    try {
      config = await api.getSystemConfig();
      error = '';
    } catch (e) {
      console.error('Failed to load Live TV settings:', e);
      error = 'Failed to load settings. Please try again.';
    } finally {
      loading = false;
    }
  }

  async function save(key: string, updates: SystemConfigUpdate) {
    savingKey = key;
    try {
      config = await api.updateSystemConfig(updates);
      showMessage('Saved');
    } catch (e) {
      console.error('Failed to save Live TV settings:', e);
      showMessage('Failed to save');
    } finally {
      savingKey = null;
    }
  }

  async function discover() {
    discovering = true;
    try {
      const result = await api.discoverLivetvTuners();
      candidates = result.candidates ?? [];
      await fetchLivetvTuners(true);
      if (candidates.length === 0 && livetv.tuners.length === 0) {
        showMessage('No HDHomeRun found on this network');
      }
    } catch (e) {
      console.error('Tuner discovery failed:', e);
      showMessage('Tuner discovery failed');
    } finally {
      discovering = false;
    }
  }

  async function addTuner(baseUrl: string) {
    if (!baseUrl.trim()) return;
    adding = true;
    try {
      const added = await api.addLivetvTuner(baseUrl.trim());
      setLivetvTuners([...livetv.tuners, added]);
      candidates = candidates.filter((c) => c.base_url !== baseUrl);
      manualUrl = '';
      showMessage('Tuner added');
    } catch (e) {
      console.error('Failed to add tuner:', e);
      showMessage('Failed to add the tuner');
    } finally {
      adding = false;
    }
  }

  async function removeTuner(id: string, force = false) {
    try {
      await api.deleteLivetvTuner(id, force);
      await fetchLivetvTuners(true);
      showMessage('Tuner removed');
    } catch (e) {
      if (e instanceof api.ApiError && e.status === 409) {
        if (confirm(`${e.message}\n\nDelete the tuner and its recordings anyway?`)) {
          await removeTuner(id, true);
        }
        return;
      }
      console.error('Failed to remove tuner:', e);
      showMessage('Failed to remove the tuner');
    }
  }

  async function rescan() {
    if (!tuner) return;
    scanning = true;
    try {
      await api.scanLivetvTuner(tuner.id);
      showMessage('Channel scan queued');
    } catch (e) {
      console.error('Failed to queue scan:', e);
      showMessage('Failed to queue the scan');
    } finally {
      scanning = false;
    }
  }

  async function syncGuide() {
    if (!tuner) return;
    try {
      await api.syncLivetvGuide(tuner.id);
      showMessage('Guide sync queued');
    } catch (e) {
      console.error('Failed to queue guide sync:', e);
      showMessage('Failed to queue the guide sync');
    }
  }

  onMount(() => {
    fetchLivetvTuners(true);
    loadConfig();
  });
</script>

<svelte:head><title>Live TV settings · kobbler</title></svelte:head>

<div class="page-container settings-livetv">
  <header class="head">
    <h1>Live TV settings</h1>
    <span class="subtitle">
      Four choices carry a recommended default — leave them and the default stands.
    </span>
  </header>

  {#if error}
    <ErrorBanner message={error} onRetry={loadConfig} />
  {/if}

  {#if loading}
    <div class="loading-card">Loading settings…</div>
  {:else}
    <div class="columns">
      <div class="column">
        <section class="card block">
          <div class="block-head">
            <span class="block-title">Tuner</span>
          </div>
          <span class="block-sub"><MetaLine items={tunerLine} /></span>

          {#if tuner}
            <div class="tuner-row">
              <TunerMeter tunerCount={livetv.tunerCount} leasesInUse={livetv.leasesInUse} compact />
              <span class="leased">
                {livetv.leasesInUse} of {livetv.tunerCount} tuners leased
              </span>
              <span class="spacer"></span>
              <button type="button" class="btn-sec small" disabled={scanning} onclick={rescan}>
                Rescan channels
              </button>
              <button type="button" class="btn-sec small" onclick={syncGuide}>Sync guide</button>
            </div>
            <span class="fine"><MetaLine items={scanLine} /></span>
            <div class="tuner-list">
              {#each livetv.tuners as t (t.id)}
                <div class="tuner-item">
                  <span class="tuner-name">{t.friendly_name || t.model_number} · {t.device_id}</span
                  >
                  <button type="button" class="btn-danger small" onclick={() => removeTuner(t.id)}>
                    Remove
                  </button>
                </div>
              {/each}
            </div>
          {:else}
            <div class="tuner-row">
              <button type="button" class="btn-primary" disabled={discovering} onclick={discover}>
                {discovering ? 'Searching…' : 'Discover tuners'}
              </button>
            </div>
            {#each candidates as candidate (candidate.device_id)}
              <div class="tuner-item">
                <span class="tuner-name">{candidate.device_id} · {candidate.local_ip}</span>
                <button
                  type="button"
                  class="btn-sec small"
                  disabled={adding || candidate.already_added}
                  onclick={() => addTuner(candidate.base_url)}
                >
                  {candidate.already_added ? 'Added' : 'Add'}
                </button>
              </div>
            {/each}
            <div class="manual">
              <input
                class="form-input"
                type="url"
                placeholder="http://192.0.2.10"
                bind:value={manualUrl}
              />
              <button
                type="button"
                class="btn-sec"
                disabled={adding || !manualUrl.trim()}
                onclick={() => addTuner(manualUrl)}
              >
                Add by URL
              </button>
            </div>
          {/if}
        </section>

        <section class="card block">
          <div class="block-head">
            <span class="block-title">Guide</span>
          </div>
          <span class="block-sub">Where programme data and artwork come from.</span>
          <div class="options" role="radiogroup" aria-label="Guide source">
            <OptionRow
              label="Tuner guide feed"
              meta={['free', '48 h', 'artwork + episode numbers']}
              selected={guideSource === 'tuner'}
              isDefault
              disabled={savingKey === 'guide'}
              onSelect={() => save('guide', { livetv_guide_source: 'tuner' })}
            />
            <OptionRow
              label="SiliconDust DVR subscription"
              meta={['~$35/yr', '14 days']}
              selected={guideSource === 'silicondust'}
              disabled={savingKey === 'guide'}
              onSelect={() => save('guide', { livetv_guide_source: 'silicondust' })}
            />
            <OptionRow
              label="XMLTV grabber"
              meta="bring your own"
              selected={guideSource === 'xmltv'}
              disabled={savingKey === 'guide'}
              onSelect={() => save('guide', { livetv_guide_source: 'xmltv' })}
            />
          </div>
          <span class="fine">
            A 48 h horizon means series rules re-evaluate on every refresh rather than scheduling
            ahead. Worth revisiting if rules start missing episodes.
          </span>
        </section>

        <section class="card block">
          <div class="block-head">
            <span class="block-title">Ad detection</span>
          </div>
          <span class="block-sub"> Breaks are always marked. What happens next is up to you. </span>
          <div class="options" role="radiogroup" aria-label="Ad detection posture">
            <OptionRow
              label="Shade breaks on the seek bar only"
              meta="no automatic action"
              selected={adPosture === 'shade'}
              isDefault
              disabled={savingKey === 'ad'}
              onSelect={() => save('ad', { livetv_ad_posture: 'shade' })}
            />
            <OptionRow
              label="Offer a Skip button in a break"
              selected={adPosture === 'button'}
              disabled={savingKey === 'ad'}
              onSelect={() => save('ad', { livetv_ad_posture: 'button' })}
            />
            <OptionRow
              label="Skip automatically, show a toast"
              selected={adPosture === 'auto'}
              disabled={savingKey === 'ad'}
              onSelect={() => save('ad', { livetv_ad_posture: 'auto' })}
            />
          </div>
          <div class="options divided">
            <ToggleRow
              label="Disable on news and live sport"
              meta="detection collapses there"
              checked={adDisableGenres.length > 0}
              disabled={savingKey === 'ad-genres'}
              onChange={(next) =>
                save('ad-genres', { livetv_ad_disable_genres: next ? AD_GENRES : [] })}
            />
            <ToggleRow
              label="Use my corrections to train the ad fingerprint database"
              meta="one fix, every future airing"
              checked={adTrain}
              disabled={savingKey === 'ad-train'}
              onChange={(next) => save('ad-train', { livetv_ad_train_from_corrections: next })}
            />
          </div>
        </section>
      </div>

      <div class="column">
        <section class="card block">
          <div class="block-head">
            <span class="block-title">Live buffer</span>
          </div>
          <span class="block-sub">
            Pause and rewind are ours to provide — the tuner has no buffer of its own.
          </span>
          <div class="options" role="radiogroup" aria-label="Live buffer policy">
            <OptionRow
              label="Keep from when you tuned in"
              selected={bufferPolicy === 'tune_in'}
              isDefault
              disabled={savingKey === 'buffer'}
              onSelect={() => save('buffer', { livetv_buffer_policy: 'tune_in' })}
            />
            <OptionRow
              label="Rolling window"
              selected={bufferPolicy === 'rolling'}
              disabled={savingKey === 'buffer'}
              onSelect={() => save('buffer', { livetv_buffer_policy: 'rolling' })}
            />
            <OptionRow
              label="Current programme and the one before"
              selected={bufferPolicy === 'programme'}
              disabled={savingKey === 'buffer'}
              onSelect={() => save('buffer', { livetv_buffer_policy: 'programme' })}
            />
          </div>
          <div class="field divided">
            <span class="field-label">Cap</span>
            <span class="spacer"></span>
            <select
              class="form-select compact"
              value={String(bufferCap)}
              disabled={savingKey === 'buffer-cap'}
              onchange={(e) =>
                save('buffer-cap', {
                  livetv_buffer_cap_hours: Number(e.currentTarget.value)
                })}
            >
              {#each [1, 2, 3, 4, 6, 8, 12] as hours (hours)}
                <option value={String(hours)}>{hours} hour{hours === 1 ? '' : 's'}</option>
              {/each}
            </select>
          </div>
        </section>

        <section class="card block">
          <div class="block-head">
            <span class="block-title">When a recording needs your tuner</span>
          </div>
          <span class="block-sub">Recordings outrank viewers. Four tuners is a hard cap.</span>
          <div class="options" role="radiogroup" aria-label="Tuner clash policy">
            <OptionRow
              label="Keep playing the buffer, disable jump-to-live"
              selected={clashPolicy === 'keep_playing'}
              isDefault
              disabled={savingKey === 'clash'}
              onSelect={() => save('clash', { livetv_clash_policy: 'keep_playing' })}
            />
            <OptionRow
              label="Stop playback"
              selected={clashPolicy === 'stop'}
              disabled={savingKey === 'clash'}
              onSelect={() => save('clash', { livetv_clash_policy: 'stop' })}
            />
            <OptionRow
              label="Ask before taking the tuner"
              selected={clashPolicy === 'ask'}
              disabled={savingKey === 'clash'}
              onSelect={() => save('clash', { livetv_clash_policy: 'ask' })}
            />
          </div>
          <span class="fine">
            If the recording is on the channel you are already watching there is no clash at all —
            the buffer just keeps growing.
          </span>
        </section>

        <section class="card block">
          <div class="block-head">
            <span class="block-title">Playback and storage</span>
          </div>
          <span class="block-sub">
            Recordings are written as HLS segments so they are watchable while still recording.
          </span>
          <div class="options">
            <ToggleRow
              label="Deinterlace and transcode for browsers"
              meta="1080i25 + AC3 cannot play in a browser"
              checked={deinterlace}
              disabled={savingKey === 'deinterlace'}
              onChange={(next) => save('deinterlace', { livetv_deinterlace_web: next })}
            />
          </div>
          <div class="field divided">
            <span class="field-label">Transcode height</span>
            <span class="spacer"></span>
            <select
              class="form-select compact"
              value={String(transcodeHeight)}
              disabled={savingKey === 'height'}
              onchange={(e) =>
                save('height', { livetv_transcode_height: Number(e.currentTarget.value) })}
            >
              {#each [480, 576, 720, 1080] as h (h)}
                <option value={String(h)}>{h}p</option>
              {/each}
            </select>
          </div>
          <div class="field">
            <span class="field-label">Recording library</span>
            <span class="spacer"></span>
            <input
              class="form-input path"
              type="text"
              placeholder="/media/livetv"
              value={recordingPath}
              disabled={savingKey === 'path'}
              onchange={(e) => save('path', { livetv_recording_path: e.currentTarget.value })}
            />
          </div>
          <span class="fine">
            Left blank, recordings follow the IPTV recording path. With that unset too they land in
            the container's data volume, which is the wrong place for hours of video.
          </span>
        </section>
      </div>
    </div>
  {/if}
</div>

<style>
  .settings-livetv {
    padding-bottom: 40px;
  }

  .head {
    display: flex;
    flex-direction: column;
    gap: 3px;
    margin-bottom: 16px;
  }

  h1 {
    margin: 0;
    font-size: 24px;
    font-weight: 600;
    letter-spacing: -0.02em;
  }

  .subtitle {
    font-size: 12px;
    color: var(--muted);
  }

  .columns {
    display: flex;
    gap: 16px;
    align-items: flex-start;
  }

  .column {
    flex: 1;
    min-width: 0;
    display: flex;
    flex-direction: column;
    gap: 14px;
  }

  .block {
    display: flex;
    flex-direction: column;
    gap: 8px;
    border-radius: 12px;
  }

  .block-head {
    display: flex;
    align-items: center;
    gap: 9px;
  }

  .block-title {
    font-size: 14px;
    font-weight: 600;
    letter-spacing: -0.01em;
  }

  .block-sub {
    font-size: 11.5px;
    color: var(--muted);
    line-height: 1.5;
  }

  .options {
    display: flex;
    flex-direction: column;
    gap: 1px;
    margin-top: 4px;
  }

  .divided {
    margin-top: 8px;
    padding-top: 12px;
    border-top: 1px solid var(--ring);
  }

  .fine {
    font-size: 11px;
    color: var(--muted);
    opacity: 0.8;
    line-height: 1.5;
    margin-top: 4px;
  }

  .field {
    display: flex;
    align-items: center;
    gap: 11px;
    min-height: 34px;
  }

  .field-label {
    font-size: 12.5px;
    color: var(--ink);
  }

  .spacer {
    flex-grow: 1;
  }

  .compact {
    width: auto;
    padding: 6px 10px;
    border-radius: 8px;
    font-size: 12px;
  }

  .path {
    width: auto;
    max-width: 260px;
    padding: 6px 10px;
    border-radius: 8px;
    font-size: 12px;
    font-family: ui-monospace, monospace;
  }

  .tuner-row {
    display: flex;
    align-items: center;
    gap: 10px;
    flex-wrap: wrap;
    margin-top: 4px;
  }

  .leased {
    font-size: 12px;
    color: var(--muted);
  }

  .small {
    padding: 6px 12px;
    font-size: 11.5px;
    border-radius: 8px;
  }

  .tuner-list {
    display: flex;
    flex-direction: column;
    gap: 6px;
  }

  .tuner-item {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 8px 0;
    border-top: 1px solid var(--ring);
  }

  .tuner-name {
    font-size: 12px;
    color: var(--muted);
    flex-grow: 1;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .manual {
    display: flex;
    align-items: center;
    gap: 8px;
  }

  .manual .form-input {
    max-width: 260px;
    padding: 8px 10px;
    border-radius: 8px;
    font-size: 12px;
  }

  @media (max-width: 900px) {
    .columns {
      flex-direction: column;
    }
  }
</style>
