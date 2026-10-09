// See https://svelte.dev/docs/kit/types#app.d.ts
// for information about these interfaces
declare global {
  namespace App {
    // interface Error {}
    // interface Locals {}
    // interface PageData {}
    // interface PageState {}
    // interface Platform {}
  }

  // Google Cast Sender SDK type stubs
  namespace chrome.cast {
    enum AutoJoinPolicy {
      ORIGIN_SCOPED = 'origin_scoped',
      TAB_AND_ORIGIN_SCOPED = 'tab_and_origin_scoped',
      PAGE_SCOPED = 'page_scoped'
    }
    class Error {
      code: string;
      description: string;
    }
    class Image {
      constructor(url: string);
      url: string;
    }
    namespace media {
      class MediaInfo {
        constructor(contentId: string, contentType: string);
        contentId: string;
        contentType: string;
        metadata: GenericMediaMetadata | null;
        customData: unknown;
      }
      class GenericMediaMetadata {
        title?: string;
        subtitle?: string;
        images?: Image[];
      }
      class LoadRequest {
        constructor(mediaInfo: MediaInfo);
        mediaInfo: MediaInfo;
      }
    }
  }

  namespace cast.framework {
    class CastContext {
      static getInstance(): CastContext;
      setOptions(options: {
        receiverApplicationId: string;
        autoJoinPolicy: chrome.cast.AutoJoinPolicy;
      }): void;
      requestSession(): Promise<void>;
      endCurrentSession(stopCasting: boolean): void;
      getCurrentSession(): CastSession | null;
      addEventListener(
        type: CastContextEventType,
        handler: (event: SessionStateEventData) => void
      ): void;
    }

    class CastSession {
      getSessionState(): SessionState;
      getCastDevice(): { friendlyName: string } | null;
      loadMedia(request: chrome.cast.media.LoadRequest): Promise<void>;
      sendMessage(namespace: string, message: unknown): Promise<void>;
      addMessageListener(namespace: string, listener: (ns: string, msg: string) => void): void;
    }

    enum CastContextEventType {
      SESSION_STATE_CHANGED = 'SESSION_STATE_CHANGED'
    }

    enum SessionState {
      NO_SESSION = 'NO_SESSION',
      SESSION_STARTING = 'SESSION_STARTING',
      SESSION_STARTED = 'SESSION_STARTED',
      SESSION_START_FAILED = 'SESSION_START_FAILED',
      SESSION_ENDING = 'SESSION_ENDING',
      SESSION_ENDED = 'SESSION_ENDED',
      SESSION_RESUMED = 'SESSION_RESUMED'
    }

    interface SessionStateEventData {
      sessionState: SessionState;
    }
  }

  // Allow access to chrome.cast on Window
  interface Window {
    chrome?: { cast?: typeof chrome.cast };
    cast?: { framework?: typeof cast.framework };
    __onGCastApiAvailable?: (available: boolean) => void;
  }
}

export {};
