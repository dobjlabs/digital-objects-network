/// <reference types="vite/client" />

interface ImportMetaEnv {
  readonly VITE_MESSAGE_BOARD_BASE_URL?: string;
}

interface ImportMeta {
  readonly env: ImportMetaEnv;
}
