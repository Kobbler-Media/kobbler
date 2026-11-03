<script lang="ts">
  import type { LoginHandler } from '#lib/types.js';

  interface Props {
    onLogin: LoginHandler;
    loading: boolean;
  }

  let { onLogin, loading }: Props = $props();

  let email = $state('');
  let password = $state('');
  let error = $state('');

  const handleSubmit = async () => {
    error = '';
    if (!email.trim() || !password) {
      error = 'Email and password required';
      return;
    }
    try {
      await onLogin(email, password);
    } catch (e) {
      error = e instanceof Error ? e.message : 'Login failed';
    }
  };
</script>

<form
  class="login-card"
  onsubmit={(e) => {
    e.preventDefault();
    handleSubmit();
  }}
>
  <h2>Sign in to Kobbler</h2>
  <p class="muted">Use your admin account for now. User logins come next.</p>
  <label>
    Email
    <input type="email" bind:value={email} autocomplete="username" />
  </label>
  <label>
    Password
    <input type="password" bind:value={password} autocomplete="current-password" />
  </label>
  {#if error}
    <div class="error">{error}</div>
  {/if}
  <button class="btn" type="submit" disabled={loading}>Sign in</button>
</form>

<style>
  /* Component-specific styles - base styles in global.css */

  .login-card {
    max-width: 380px;
    margin: 60px auto 0;
    padding: 24px;
    background: var(--card);
    border: 1px solid var(--ring);
    border-radius: var(--radius, 18px);
    display: grid;
    gap: 12px;
  }

  .login-card label {
    display: grid;
    gap: 6px;
    font-size: 13px;
    color: var(--muted);
  }

  .login-card input {
    padding: 8px 10px;
    border-radius: 10px;
    border: 1px solid var(--ring);
    background: transparent;
    color: var(--ink);
  }

  .muted {
    font-size: 13px;
  }

  .error {
    color: #d06b5b;
    font-size: 13px;
  }

  /* Custom button styles - pill shape with uppercase */
  .btn {
    border-radius: 999px;
    padding: 8px 14px;
    border: 1px solid #3b2b1f;
    background: var(--accent);
    color: #1d130c;
    font-size: 12px;
    letter-spacing: 0.6px;
    text-transform: uppercase;
  }
</style>
