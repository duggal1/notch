/// <reference types="vite/client" />

interface Window {
  /** Injected by the host before document start (native shells). */
  __NOTCH__?: { w: number; h: number };
  /** Set by the UI once mounted — the host's "loaded" signal. */
  __ready?: boolean;
}
