// ─── Types ───────────────────────────────────────────────────────────────────
export type {
  MuxSpecificationVersion,
  MuxSessionInfo,
  ActiveWindow,
  SidebarPane,
  SidebarPosition,
  MuxProviderMetadata,
  MuxProviderV1,
  WindowCapable,
  SidebarCapable,
  BatchCapable,
  AsyncReadCapable,
  FullMuxProvider,
  MuxProvider,
  MuxProviderSettings,
} from "./types";

// ─── Type guards ─────────────────────────────────────────────────────────────
export {
  isWindowCapable,
  isSidebarCapable,
  isBatchCapable,
  isAsyncReadCapable,
  isFullSidebarCapable,
} from "./types";
