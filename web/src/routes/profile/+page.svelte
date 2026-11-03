<script lang="ts">
  import { onMount } from 'svelte';
  import { getAuthState, setAvatarUrl } from '#lib/stores/auth.svelte.js';
  import { getUIState, setTheme } from '#lib/stores/ui.svelte.js';
  import AvatarCropper from '#lib/components/AvatarCropper.svelte';
  import { checkAuth } from '#lib/services/data.svelte.js';
  import { createWebPushController } from '#lib/services/webpush.svelte.js';
  import {
    getMyNotificationPrefs,
    updateMyNotificationPrefs,
    updateMyTimezone
  } from '#lib/api/client.js';
  import type {
    ChatNotificationPrefs,
    ChatNotificationPrefsUpdate,
    MuteDuration,
    NotificationTrigger
  } from '#lib/types.js';

  const authState = getAuthState();
  const ui = getUIState();
  const apiBase = import.meta.env.VITE_API_BASE_URL || '';

  let currentPassword = $state('');
  let newPassword = $state('');
  let confirmPassword = $state('');
  let passwordMsg = $state('');
  let passwordError = $state(false);
  let passwordSaving = $state(false);

  const webPush = createWebPushController();

  // Every zone the browser knows, so nothing here has to be kept in step with
  // the tz database by hand.
  const zoneNames: string[] =
    typeof Intl.supportedValuesOf === 'function' ? Intl.supportedValuesOf('timeZone') : [];
  // Read after mount, never at module scope: on a prerendered page that would be
  // the build machine's clock, which is the exact confusion this setting fixes.
  let browserZone = $state('');

  // Writable derived: it tracks whatever /me last said, and a save can set it
  // early so the picker does not lag the round trip.
  let timezone = $derived(authState.timezone);
  let timezoneMsg = $state('');
  let timezoneError = $state(false);
  let timezoneSaving = $state(false);

  onMount(() => {
    webPush.refresh();
    browserZone = Intl.DateTimeFormat().resolvedOptions().timeZone;
    loadChatPrefs();
  });

  async function saveTimezone(value: string) {
    timezoneSaving = true;
    timezoneMsg = '';
    timezoneError = false;
    try {
      await updateMyTimezone(value);
      timezone = value;
      await checkAuth();
      timezoneMsg = value ? `Quiet hours now follow ${value}.` : 'Back on the server clock.';
    } catch (e) {
      timezoneMsg = e instanceof Error ? e.message : 'Failed to save timezone';
      timezoneError = true;
    } finally {
      timezoneSaving = false;
    }
  }

  const chatTriggerOptions: { id: NotificationTrigger; label: string }[] = [
    { id: 'on_grab', label: 'A download starting' },
    { id: 'on_import', label: 'Something finishing and arriving' },
    { id: 'on_failed', label: 'A search giving up' },
    { id: 'on_media_corrupt', label: 'A file turning out to be corrupt' }
  ];

  const muteOptions: { id: MuteDuration; label: string }[] = [
    { id: 'an_hour', label: 'An hour' },
    { id: 'a_couple_of_hours', label: 'A couple of hours' },
    { id: 'the_rest_of_today', label: 'The rest of today' },
    { id: 'until_tomorrow', label: 'Until tomorrow morning' },
    { id: 'a_week', label: 'A week' }
  ];

  let chatPrefs = $state<ChatNotificationPrefs | null>(null);
  let chatPrefsLoadError = $state('');
  let chatPrefsMsg = $state('');
  let chatPrefsError = $state(false);
  let chatPrefsBusy = $state(false);
  let muteDuration = $state<MuteDuration>('a_couple_of_hours');
  // Seeded with the server's defaults so the inputs are never blank while the
  // window they show is switched off.
  let quietFrom = $state('22:00');
  let quietTo = $state('07:00');

  const quietHoursOn = $derived(chatPrefs !== null && chatPrefs.quiet_from !== null);
  const quietWrapsMidnight = $derived(quietFrom > quietTo);
  const quietTimesDirty = $derived(
    chatPrefs !== null && (quietFrom !== chatPrefs.quiet_from || quietTo !== chatPrefs.quiet_to)
  );

  function adoptChatPrefs(prefs: ChatNotificationPrefs) {
    chatPrefs = prefs;
    if (prefs.quiet_from && prefs.quiet_to) {
      quietFrom = prefs.quiet_from;
      quietTo = prefs.quiet_to;
    }
  }

  async function loadChatPrefs() {
    try {
      adoptChatPrefs(await getMyNotificationPrefs());
      chatPrefsLoadError = '';
    } catch (e) {
      chatPrefsLoadError =
        e instanceof Error ? e.message : 'Failed to load chat notification settings';
    }
  }

  async function changeChatPrefs(update: ChatNotificationPrefsUpdate) {
    chatPrefsBusy = true;
    chatPrefsError = false;
    chatPrefsMsg = '';
    try {
      const next = await updateMyNotificationPrefs(update);
      adoptChatPrefs(next);
      chatPrefsMsg = next.summary;
    } catch (e) {
      chatPrefsMsg = e instanceof Error ? e.message : 'Failed to save';
      chatPrefsError = true;
    } finally {
      chatPrefsBusy = false;
    }
  }

  // A zero-width window is how "no quiet hours" is stored, and the server spells
  // it as its own action rather than accepting from == to.
  function saveQuietHours() {
    if (quietFrom === quietTo) {
      chatPrefsMsg = 'Start and end must differ. Switch quiet hours off to be reachable all day.';
      chatPrefsError = true;
      return;
    }
    changeChatPrefs({ action: 'set_quiet_hours', quiet_from: quietFrom, quiet_to: quietTo });
  }

  function toggleQuietHours(on: boolean) {
    if (on) {
      saveQuietHours();
    } else {
      changeChatPrefs({ action: 'clear_quiet_hours' });
    }
  }

  function toggleChatTrigger(id: NotificationTrigger, on: boolean) {
    changeChatPrefs({ action: on ? 'enable_trigger' : 'disable_trigger', trigger: id });
  }

  // An absolute instant, shown on the clock the preferences are read against
  // rather than on whichever one this browser happens to be sitting in.
  function formatMutedUntil(iso: string, zone: string | null): string {
    const when = new Date(iso);
    if (Number.isNaN(when.getTime())) return iso;
    const options: Intl.DateTimeFormatOptions = {
      weekday: 'short',
      hour: '2-digit',
      minute: '2-digit'
    };
    try {
      return when.toLocaleString(undefined, zone ? { ...options, timeZone: zone } : options);
    } catch {
      return when.toLocaleString(undefined, options);
    }
  }

  async function handleAvatarSaved() {
    await checkAuth();
  }

  async function handleDeleteAvatar() {
    try {
      const resp = await fetch(`${apiBase}/api/v1/admin/me/avatar`, {
        method: 'DELETE',
        credentials: 'include'
      });
      if (resp.ok) {
        setAvatarUrl('');
        checkAuth();
      }
    } catch {
      // ignore
    }
  }

  async function handlePasswordChange() {
    passwordMsg = '';
    passwordError = false;

    if (newPassword.length < 8) {
      passwordMsg = 'Password must be at least 8 characters';
      passwordError = true;
      return;
    }

    if (newPassword !== confirmPassword) {
      passwordMsg = 'Passwords do not match';
      passwordError = true;
      return;
    }

    passwordSaving = true;
    try {
      const resp = await fetch(`${apiBase}/api/v1/admin/me/password`, {
        method: 'PUT',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({
          current_password: currentPassword,
          new_password: newPassword
        }),
        credentials: 'include'
      });

      if (resp.ok) {
        passwordMsg = 'Password updated';
        passwordError = false;
        currentPassword = '';
        newPassword = '';
        confirmPassword = '';
      } else {
        passwordMsg = await resp.text();
        passwordError = true;
      }
    } catch {
      passwordMsg = 'Failed to update password';
      passwordError = true;
    } finally {
      passwordSaving = false;
    }
  }

  let pinCode = $state('');
  let pinApiUrl = $state(typeof window !== 'undefined' ? window.location.origin : '');
  let pinMsg = $state('');
  let pinError = $state(false);
  let pinSaving = $state(false);

  // Profile PIN (device-switch gate)
  let profilePin = $state('');
  let profilePinConfirm = $state('');
  let profilePinMsg = $state('');
  let profilePinError = $state(false);
  let profilePinSaving = $state(false);

  async function handleSetProfilePin() {
    profilePinMsg = '';
    profilePinError = false;

    const pin = profilePin.trim();
    if (pin.length < 3) {
      profilePinMsg = 'PIN must be at least 3 characters';
      profilePinError = true;
      return;
    }
    if (/\s/.test(pin)) {
      profilePinMsg = 'PIN must not contain spaces';
      profilePinError = true;
      return;
    }
    if (pin !== profilePinConfirm.trim()) {
      profilePinMsg = 'PINs do not match';
      profilePinError = true;
      return;
    }

    profilePinSaving = true;
    try {
      const resp = await fetch(`${apiBase}/api/v1/admin/me/profile-pin`, {
        method: 'PUT',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ pin }),
        credentials: 'include'
      });
      if (resp.ok) {
        profilePinMsg = authState.hasPin ? 'Profile PIN updated' : 'Profile PIN set';
        profilePinError = false;
        profilePin = '';
        profilePinConfirm = '';
        await checkAuth();
      } else {
        profilePinMsg = await resp.text();
        profilePinError = true;
      }
    } catch {
      profilePinMsg = 'Failed to update profile PIN';
      profilePinError = true;
    } finally {
      profilePinSaving = false;
    }
  }

  async function handleClearProfilePin() {
    if (!confirm('Remove the PIN protecting this profile on all devices?')) return;
    profilePinMsg = '';
    profilePinError = false;
    profilePinSaving = true;
    try {
      const resp = await fetch(`${apiBase}/api/v1/admin/me/profile-pin`, {
        method: 'DELETE',
        credentials: 'include'
      });
      if (resp.ok) {
        profilePinMsg = 'Profile PIN removed';
        profilePinError = false;
        profilePin = '';
        profilePinConfirm = '';
        await checkAuth();
      } else {
        profilePinMsg = await resp.text();
        profilePinError = true;
      }
    } catch {
      profilePinMsg = 'Failed to remove profile PIN';
      profilePinError = true;
    } finally {
      profilePinSaving = false;
    }
  }

  async function handlePinLogin() {
    pinMsg = '';
    pinError = false;

    // The PIN service owns the format and its length is configurable there, so
    // only sanity-check the shape - a fixed length breaks on every change.
    const pin = pinCode.trim().toUpperCase();
    if (pin.length < 4 || pin.length > 32) {
      pinMsg = 'PIN must be 4-32 characters';
      pinError = true;
      return;
    }

    if (!pinApiUrl.trim()) {
      pinMsg = 'API URL is required';
      pinError = true;
      return;
    }

    pinSaving = true;
    try {
      const resp = await fetch(`${apiBase}/api/v1/admin/pin-login`, {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ pin, api_url: pinApiUrl.trim() }),
        credentials: 'include'
      });

      if (resp.ok) {
        pinMsg = 'App linked successfully';
        pinError = false;
        pinCode = '';
      } else {
        pinMsg = await resp.text();
        pinError = true;
      }
    } catch {
      pinMsg = 'Failed to link app';
      pinError = true;
    } finally {
      pinSaving = false;
    }
  }

  const roleBadgeClass = $derived(
    authState.role === 'admin'
      ? 'role-admin'
      : authState.role === 'contributor'
        ? 'role-contributor'
        : 'role-viewer'
  );
</script>

<svelte:head>
  <title>Profile — Kobbler</title>
</svelte:head>

<div class="profile-page">
  <h1 class="page-title">Profile</h1>

  <div class="profile-grid">
    <section class="card avatar-section">
      <h2 class="card-title">Avatar</h2>
      <div class="avatar-content">
        <AvatarCropper currentAvatarUrl={authState.avatarUrl} onSaved={handleAvatarSaved} />
        {#if authState.avatarUrl}
          <button type="button" class="btn-remove" onclick={handleDeleteAvatar}
            >Remove avatar</button
          >
        {/if}
      </div>
    </section>

    <section class="card info-section">
      <h2 class="card-title">Account</h2>
      <div class="info-row">
        <span class="info-label">Email</span>
        <span class="info-value">{authState.email}</span>
      </div>
      <div class="info-row">
        <span class="info-label">Role</span>
        <span class="role-badge {roleBadgeClass}">{authState.role}</span>
      </div>
    </section>

    <section class="card preferences-section">
      <h2 class="card-title">Preferences</h2>
      <div class="info-row">
        <div class="notify-copy">
          <span class="info-label">Timezone</span>
          <p class="notify-hint">
            Quiet hours and "until tomorrow" are read on this clock. Unset means the server's.
          </p>
          {#if timezoneMsg}
            <p class="notify-hint" class:notify-error={timezoneError}>{timezoneMsg}</p>
          {/if}
        </div>
        <div class="timezone-controls">
          {#if zoneNames.length > 0}
            <select
              class="timezone-select"
              value={timezone}
              disabled={timezoneSaving}
              onchange={(e) => saveTimezone(e.currentTarget.value)}
            >
              <option value="">Server default</option>
              {#each zoneNames as zone (zone)}
                <option value={zone}>{zone}</option>
              {/each}
            </select>
          {:else}
            <input
              class="timezone-select"
              type="text"
              bind:value={timezone}
              placeholder="Europe/London"
              disabled={timezoneSaving}
              onblur={() => saveTimezone(timezone)}
            />
          {/if}
          {#if browserZone && browserZone !== timezone}
            <button
              type="button"
              class="theme-btn"
              disabled={timezoneSaving}
              onclick={() => saveTimezone(browserZone)}
            >
              Use {browserZone}
            </button>
          {/if}
        </div>
      </div>
      <div class="info-row">
        <span class="info-label">Theme</span>
        <div class="theme-toggle">
          <button
            type="button"
            class="theme-btn"
            class:active={ui.theme === 'dark'}
            onclick={() => setTheme('dark')}
          >
            <svg viewBox="0 0 24 24" width="14" height="14" fill="currentColor">
              <path
                d="M12 3a9 9 0 1 0 9 9c0-.46-.04-.92-.1-1.36a5.389 5.389 0 0 1-4.4 2.26 5.403 5.403 0 0 1-3.14-9.8c-.44-.06-.9-.1-1.36-.1z"
              ></path>
            </svg>
            Dark
          </button>
          <button
            type="button"
            class="theme-btn"
            class:active={ui.theme === 'light'}
            onclick={() => setTheme('light')}
          >
            <svg viewBox="0 0 24 24" width="14" height="14" fill="currentColor">
              <path
                d="M12 7a5 5 0 1 0 0 10 5 5 0 0 0 0-10zm0-3a1 1 0 0 0 1-1V1a1 1 0 1 0-2 0v2a1 1 0 0 0 1 1zm0 18a1 1 0 0 0-1 1v2a1 1 0 1 0 2 0v-2a1 1 0 0 0-1-1zM4.22 5.64l1.42-1.42a1 1 0 0 0-1.42-1.42L2.8 4.22a1 1 0 0 0 1.42 1.42zm15.56 12.72l-1.42 1.42a1 1 0 1 0 1.42 1.42l1.42-1.42a1 1 0 0 0-1.42-1.42zM4 12a1 1 0 0 0-1-1H1a1 1 0 1 0 0 2h2a1 1 0 0 0 1-1zm19 0a1 1 0 0 0-1-1h-2a1 1 0 1 0 0 2h2a1 1 0 0 0 1-1zM5.64 19.78l-1.42 1.42a1 1 0 1 0 1.42 1.42l1.42-1.42a1 1 0 0 0-1.42-1.42zm12.72-15.56l1.42-1.42a1 1 0 0 0-1.42-1.42l-1.42 1.42a1 1 0 1 0 1.42 1.42z"
              ></path>
            </svg>
            Light
          </button>
        </div>
      </div>
    </section>

    <section class="card chat-notifications-section">
      <h2 class="card-title">Chat notifications</h2>
      <p class="pin-description">
        What the download bot tells you about, and when it is allowed to. Anything here can also be
        changed by asking it.
      </p>

      {#if chatPrefsLoadError}
        <p class="notify-error">{chatPrefsLoadError}</p>
      {:else if !chatPrefs}
        <p class="notify-hint">Loading...</p>
      {:else}
        <div class="info-row">
          <div class="notify-copy">
            <span class="info-label">Messages</span>
            <p class="notify-hint">{chatPrefs.summary}</p>
          </div>
          <div class="theme-toggle">
            <button
              type="button"
              class="theme-btn"
              class:active={chatPrefs.enabled}
              disabled={chatPrefsBusy}
              onclick={() => changeChatPrefs({ action: 'resume' })}
            >
              On
            </button>
            <button
              type="button"
              class="theme-btn"
              class:active={!chatPrefs.enabled}
              disabled={chatPrefsBusy}
              onclick={() => changeChatPrefs({ action: 'mute', duration: 'indefinitely' })}
            >
              Off
            </button>
          </div>
        </div>

        <div class="info-row">
          <div class="notify-copy">
            <span class="info-label">Tell me about</span>
            <p class="notify-hint">
              Only "something finishing" to begin with — a chatty bot gets muted in week one.
            </p>
          </div>
          <div class="check-list">
            {#each chatTriggerOptions as option (option.id)}
              <label class="check">
                <input
                  type="checkbox"
                  checked={chatPrefs.triggers.includes(option.id)}
                  disabled={chatPrefsBusy}
                  onchange={(e) => toggleChatTrigger(option.id, e.currentTarget.checked)}
                />
                <span>{option.label}</span>
              </label>
            {/each}
          </div>
        </div>

        <div class="info-row">
          <div class="notify-copy">
            <span class="info-label">Quiet hours</span>
            {#if chatPrefs.quiet_from && chatPrefs.quiet_to}
              <p class="notify-hint">
                Nothing between {chatPrefs.quiet_from} and {chatPrefs.quiet_to}{quietWrapsMidnight
                  ? ', crossing midnight'
                  : ''}, read on the {chatPrefs.timezone_label} clock.
              </p>
            {:else}
              <p class="notify-hint">Off — messages can arrive at any hour of the day or night.</p>
            {/if}
          </div>
          <div class="quiet-controls">
            <label class="check">
              <input
                type="checkbox"
                checked={quietHoursOn}
                disabled={chatPrefsBusy}
                onchange={(e) => toggleQuietHours(e.currentTarget.checked)}
              />
              <span>{quietHoursOn ? 'On' : 'Off'}</span>
            </label>
            <div class="quiet-times">
              <input
                class="timezone-select time-input"
                type="time"
                bind:value={quietFrom}
                disabled={chatPrefsBusy || !quietHoursOn}
                aria-label="Quiet hours start"
              />
              <span class="quiet-sep">to</span>
              <input
                class="timezone-select time-input"
                type="time"
                bind:value={quietTo}
                disabled={chatPrefsBusy || !quietHoursOn}
                aria-label="Quiet hours end"
              />
              <button
                type="button"
                class="theme-btn"
                disabled={chatPrefsBusy || !quietHoursOn || !quietTimesDirty}
                onclick={saveQuietHours}
              >
                Save
              </button>
            </div>
          </div>
        </div>

        <div class="info-row last-row">
          <div class="notify-copy">
            <span class="info-label">Mute</span>
            {#if chatPrefs.muted && chatPrefs.muted_until}
              <p class="notify-hint">
                Muted until {formatMutedUntil(chatPrefs.muted_until, chatPrefs.timezone)}.
              </p>
            {:else}
              <p class="notify-hint">Silence it for a while without changing anything else.</p>
            {/if}
          </div>
          <div class="timezone-controls">
            {#if chatPrefs.muted}
              <button
                type="button"
                class="theme-btn"
                disabled={chatPrefsBusy}
                onclick={() => changeChatPrefs({ action: 'resume' })}
              >
                Resume now
              </button>
            {:else}
              <select class="timezone-select" bind:value={muteDuration} disabled={chatPrefsBusy}>
                {#each muteOptions as option (option.id)}
                  <option value={option.id}>{option.label}</option>
                {/each}
              </select>
              <button
                type="button"
                class="theme-btn"
                disabled={chatPrefsBusy}
                onclick={() => changeChatPrefs({ action: 'mute', duration: muteDuration })}
              >
                Mute
              </button>
            {/if}
          </div>
        </div>

        {#if chatPrefsMsg}
          <p class="chat-prefs-msg" class:notify-error={chatPrefsError}>{chatPrefsMsg}</p>
        {/if}
      {/if}
    </section>

    <section class="card notifications-section">
      <h2 class="card-title">Notifications</h2>
      {#if !webPush.supported}
        <p class="notify-hint">Push notifications are not supported in this browser.</p>
      {:else}
        <div class="info-row">
          <div class="notify-copy">
            <span class="info-label">Browser push</span>
            <p class="notify-hint">
              Get a notification in this browser when something new is available to watch.
            </p>
            {#if webPush.error}
              <p class="notify-error">{webPush.error}</p>
            {/if}
          </div>
          <button
            type="button"
            class="theme-btn"
            class:active={webPush.subscribed}
            disabled={webPush.loading}
            onclick={() => (webPush.subscribed ? webPush.unsubscribe() : webPush.subscribe())}
          >
            {webPush.loading ? 'Working...' : webPush.subscribed ? 'Turn off' : 'Turn on'}
          </button>
        </div>
      {/if}
    </section>

    <section class="card password-section">
      <h2 class="card-title">Change Password</h2>
      <form
        class="password-form"
        onsubmit={(e) => {
          e.preventDefault();
          handlePasswordChange();
        }}
      >
        <label class="field">
          <span class="field-label">Current Password</span>
          <input type="password" bind:value={currentPassword} autocomplete="current-password" />
        </label>
        <label class="field">
          <span class="field-label">New Password</span>
          <input type="password" bind:value={newPassword} autocomplete="new-password" />
        </label>
        <label class="field">
          <span class="field-label">Confirm New Password</span>
          <input type="password" bind:value={confirmPassword} autocomplete="new-password" />
        </label>
        {#if passwordMsg}
          <p class="password-msg" class:error={passwordError} class:success={!passwordError}>
            {passwordMsg}
          </p>
        {/if}
        <button
          type="submit"
          class="btn-primary"
          disabled={passwordSaving || !currentPassword || !newPassword || !confirmPassword}
        >
          {passwordSaving ? 'Updating...' : 'Update Password'}
        </button>
      </form>
    </section>

    <section class="card profile-pin-section">
      <h2 class="card-title">Profile PIN</h2>
      <p class="pin-description">
        {#if authState.hasPin}
          A PIN is set. Anyone switching into your profile on a shared device will be asked for it.
        {:else}
          Optional. Set a PIN and anyone switching into your profile on a shared device (phone, TV)
          will be asked for it before they can see your library and watch history.
        {/if}
      </p>
      <form
        class="password-form"
        onsubmit={(e) => {
          e.preventDefault();
          handleSetProfilePin();
        }}
      >
        <label class="field">
          <span class="field-label">{authState.hasPin ? 'New PIN' : 'PIN'}</span>
          <input
            type="password"
            inputmode="numeric"
            autocomplete="off"
            bind:value={profilePin}
            maxlength="32"
            placeholder="3+ characters"
          />
        </label>
        <label class="field">
          <span class="field-label">Confirm PIN</span>
          <input
            type="password"
            inputmode="numeric"
            autocomplete="off"
            bind:value={profilePinConfirm}
            maxlength="32"
          />
        </label>
        {#if profilePinMsg}
          <p class="password-msg" class:error={profilePinError} class:success={!profilePinError}>
            {profilePinMsg}
          </p>
        {/if}
        <div class="profile-pin-actions">
          <button
            type="submit"
            class="btn-primary"
            disabled={profilePinSaving || !profilePin || !profilePinConfirm}
          >
            {profilePinSaving ? 'Saving...' : authState.hasPin ? 'Update PIN' : 'Set PIN'}
          </button>
          {#if authState.hasPin}
            <button
              type="button"
              class="btn-remove"
              onclick={handleClearProfilePin}
              disabled={profilePinSaving}
            >
              Remove PIN
            </button>
          {/if}
        </div>
      </form>
    </section>

    <section class="card pin-section">
      <h2 class="card-title">Link App</h2>
      <p class="pin-description">Enter the PIN shown in your app to connect it to this account.</p>
      <form
        class="pin-form"
        onsubmit={(e) => {
          e.preventDefault();
          handlePinLogin();
        }}
      >
        <label class="field">
          <span class="field-label">PIN</span>
          <input
            type="text"
            class="pin-input"
            bind:value={pinCode}
            maxlength="32"
            placeholder="K7M2P9XRJ4"
            autocomplete="off"
            oninput={(e) => {
              const target = e.currentTarget;
              target.value = target.value.toUpperCase();
              pinCode = target.value;
            }}
          />
        </label>
        <label class="field">
          <span class="field-label">API URL</span>
          <input type="text" bind:value={pinApiUrl} placeholder="https://..." />
        </label>
        {#if pinMsg}
          <p class="password-msg" class:error={pinError} class:success={!pinError}>
            {pinMsg}
          </p>
        {/if}
        <button
          type="submit"
          class="btn-primary"
          disabled={pinSaving || pinCode.trim().length < 4 || !pinApiUrl.trim()}
        >
          {pinSaving ? 'Linking...' : 'Link App'}
        </button>
      </form>
    </section>
  </div>
</div>

<style>
  .profile-page {
    max-width: 640px;
    margin: 0 auto;
    padding: 24px 20px;
  }

  .page-title {
    font-size: 22px;
    font-weight: 600;
    margin: 0 0 24px;
  }

  .profile-grid {
    display: flex;
    flex-direction: column;
    gap: 16px;
  }

  .card {
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: 12px;
    padding: 20px;
  }

  .card-title {
    font-size: 14px;
    font-weight: 600;
    margin: 0 0 16px;
    color: var(--ink);
  }

  .avatar-content {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 12px;
  }

  .btn-remove {
    font-size: 12px;
    color: var(--danger);
    background: none;
    border: none;
    cursor: pointer;
    padding: 0;
  }

  .btn-remove:hover {
    text-decoration: underline;
  }

  .btn-remove:disabled {
    opacity: 0.5;
    cursor: not-allowed;
    text-decoration: none;
  }

  .profile-pin-actions {
    display: flex;
    align-items: center;
    gap: 16px;
  }

  .info-row {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 10px 0;
    border-bottom: 1px solid var(--ring);
  }

  .info-row:last-child {
    border-bottom: none;
  }

  .info-label {
    font-size: 13px;
    color: var(--muted);
  }

  .info-value {
    font-size: 13px;
    color: var(--ink);
  }

  .notify-copy {
    display: flex;
    flex-direction: column;
    gap: 4px;
    padding-right: 16px;
  }

  .notify-hint {
    margin: 0;
    font-size: 12px;
    color: var(--muted);
  }

  .notify-error {
    margin: 0;
    font-size: 12px;
    color: var(--danger);
  }

  .check-list {
    display: flex;
    flex-direction: column;
    align-items: flex-start;
    gap: 6px;
  }

  .check {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    font-size: 12px;
    color: var(--ink);
    cursor: pointer;
  }

  .check input {
    accent-color: var(--accent);
    cursor: pointer;
  }

  .check input:disabled {
    cursor: not-allowed;
  }

  .quiet-controls {
    display: flex;
    flex-direction: column;
    align-items: flex-end;
    gap: 8px;
  }

  .quiet-times {
    display: flex;
    align-items: center;
    gap: 6px;
    flex-wrap: wrap;
    justify-content: flex-end;
  }

  .quiet-sep {
    font-size: 12px;
    color: var(--muted);
  }

  .time-input {
    max-width: 110px;
  }

  .last-row {
    border-bottom: none;
  }

  .chat-prefs-msg {
    margin: 12px 0 0;
    font-size: 12px;
    color: var(--muted);
  }

  .timezone-controls {
    display: flex;
    align-items: center;
    gap: 8px;
    flex-wrap: wrap;
    justify-content: flex-end;
  }

  .timezone-select {
    padding: 6px 10px;
    border: 1px solid var(--ring);
    border-radius: 8px;
    background: var(--bg);
    color: var(--ink);
    font-size: 13px;
    max-width: 220px;
    outline: none;
  }

  .timezone-select:focus {
    border-color: var(--accent);
  }

  .role-badge {
    font-size: 11px;
    font-weight: 600;
    padding: 2px 8px;
    border-radius: 10px;
    text-transform: capitalize;
  }

  .role-admin {
    background: rgba(211, 140, 90, 0.15);
    color: var(--accent);
  }

  .role-contributor {
    background: rgba(34, 197, 94, 0.15);
    color: #22c55e;
  }

  .role-viewer {
    background: rgba(156, 163, 175, 0.15);
    color: var(--muted);
  }

  .theme-toggle {
    display: flex;
    gap: 6px;
  }

  .theme-btn {
    display: inline-flex;
    align-items: center;
    gap: 6px;
    padding: 6px 14px;
    border: 1px solid var(--ring);
    border-radius: 8px;
    background: transparent;
    color: var(--muted);
    font-size: 12px;
    font-weight: 500;
    cursor: pointer;
    transition:
      background 0.15s,
      border-color 0.15s,
      color 0.15s;
  }

  .theme-btn:hover {
    border-color: var(--accent);
    color: var(--ink);
  }

  .theme-btn.active {
    background: var(--accent-dim);
    border-color: var(--accent);
    color: var(--accent);
  }

  .password-form {
    display: flex;
    flex-direction: column;
    gap: 14px;
  }

  .field {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }

  .field-label {
    font-size: 12px;
    color: var(--muted);
  }

  .field input {
    padding: 8px 12px;
    border: 1px solid var(--ring);
    border-radius: 8px;
    background: var(--bg);
    color: var(--ink);
    font-size: 14px;
    outline: none;
    transition: border-color 0.15s;
  }

  .field input:focus {
    border-color: var(--accent);
  }

  .password-msg {
    font-size: 12px;
    margin: 0;
  }

  .password-msg.error {
    color: var(--danger);
  }

  .password-msg.success {
    color: var(--success);
  }

  .btn-primary {
    align-self: flex-start;
    padding: 8px 20px;
    border-radius: 8px;
    border: none;
    background: var(--accent);
    color: var(--bg);
    font-size: 13px;
    font-weight: 500;
    cursor: pointer;
    transition: opacity 0.15s;
  }

  .btn-primary:hover {
    opacity: 0.9;
  }

  .btn-primary:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }

  .pin-description {
    font-size: 12px;
    color: var(--muted);
    margin: 0 0 14px;
    line-height: 1.5;
  }

  .pin-form {
    display: flex;
    flex-direction: column;
    gap: 14px;
  }

  .pin-input {
    font-family: monospace;
    font-size: 18px;
    letter-spacing: 2px;
    text-transform: uppercase;
    max-width: 260px;
    text-align: center;
  }

  @media (max-width: 768px) {
    .profile-page {
      padding: 16px 12px;
    }
  }
</style>
