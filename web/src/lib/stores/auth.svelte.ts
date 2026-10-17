// Authentication store using Svelte 5 runes
// Uses httpOnly cookies - no token stored in localStorage

export type UserRole = 'admin' | 'contributor' | 'viewer' | '';

let isAuthed = $state(false);
let role = $state<UserRole>('');
let email = $state('');
let avatarUrl = $state('');
let hasPin = $state(false);
let isKids = $state(false);
let timezone = $state('');
let castReceiverAppId = $state('');
let loginError = $state('');
let loginLoading = $state(false);

export function getAuthState() {
  return {
    get isAuthed() {
      return isAuthed;
    },
    get role() {
      return role;
    },
    get isAdmin() {
      return role === 'admin';
    },
    get canManageContent() {
      return role === 'admin' || role === 'contributor';
    },
    get email() {
      return email;
    },
    get avatarUrl() {
      return avatarUrl;
    },
    get hasPin() {
      return hasPin;
    },
    get isKids() {
      return isKids;
    },
    get timezone() {
      return timezone;
    },
    get castReceiverAppId() {
      return castReceiverAppId;
    },
    get loginError() {
      return loginError;
    },
    get loginLoading() {
      return loginLoading;
    }
  };
}

export function setAuthed(value: boolean) {
  isAuthed = value;
}

export function setRole(r: string) {
  role = r as UserRole;
}

export function setEmail(e: string) {
  email = e;
}

export function setAvatarUrl(url: string) {
  avatarUrl = url;
}

export function setHasPin(value: boolean) {
  hasPin = value;
}

export function setIsKids(value: boolean) {
  isKids = value;
}

export function setTimezone(value: string) {
  timezone = value;
}

export function setCastReceiverAppId(id: string) {
  castReceiverAppId = id;
}

export function setLoginError(error: string) {
  loginError = error;
}

export function setLoginLoading(loading: boolean) {
  loginLoading = loading;
}

export function clearAuth() {
  isAuthed = false;
  role = '';
  email = '';
  avatarUrl = '';
  hasPin = false;
  isKids = false;
  timezone = '';
  castReceiverAppId = '';
  loginError = '';
  loginLoading = false;
}
