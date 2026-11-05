/// <reference types="vite/client" />

import type { HydraAPI } from './bridge/hydraApi'

declare global {
  interface Window {
    hydra: HydraAPI
  }

  interface File {
    /**
     * Electron added the absolute path of dropped files here. Browsers and Tauri webviews do not: it is always
     * `undefined` there, and OS file drops need Tauri's drag-drop event instead (G2).
     */
    path?: string
  }
}
