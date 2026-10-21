// Typed wrappers around the Tauri commands and events of the Rust core.
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export interface AppInfo {
  version: string;
  os: string;
  arch: string;
  profile: string;
  registry: boolean;
  server: string | null;
}

export interface Contact {
  id: number;
  handle: string;
  displayName: string | null;
  verified: boolean;
  online: boolean;
  safetyNumber: string;
  keyChanged: boolean;
  nameUnconfirmed: boolean;
  unread: number;
}

export type FileState = "offered" | "downloading" | "paused" | "waiting" | "complete" | "failed";

export interface FileTransfer {
  fileId: string;
  name: string;
  size: number;
  outgoing: boolean;
  state: FileState;
  bytes: number;
  rate: number;
  openable: boolean;
}

export type MessageStatus = "pending" | "sent" | "delivered" | "read" | "received";

export interface Message {
  id: number;
  msgId: string;
  outgoing: boolean;
  sentAt: number;
  status: MessageStatus;
  body: string;
  file: FileTransfer | null;
}

export interface Settings {
  readReceipts: boolean;
  discoverable: boolean | null;
  email: string | null;
  emailVerified: boolean | null;
  backupEnabled: boolean;
  accountMissing: boolean;
  sounds: boolean;
}

export interface Invite {
  handle: string;
  code: string;
  qrSvg: string;
}

export type OwEvent =
  | { type: "message"; contact: number; message: Message }
  | { type: "status"; contact: number; msgId: string; status: MessageStatus }
  | { type: "contacts" }
  | { type: "presence"; contact: number; online: boolean }
  | { type: "keyChanged"; contact: number }
  | { type: "contactRequest"; contact: number }
  | { type: "file"; contact: number; file: FileTransfer }
  | { type: "resync" };

export const api = {
  appInfo: () => invoke<AppInfo>("app_info"),
  identityStatus: () => invoke<string | null>("identity_status"),
  createIdentity: (name: string, deviceName: string, email: string | null, discoverable: boolean) =>
    invoke<string[]>("create_identity", { name, deviceName, email, discoverable }),
  recoveryStart: (name: string) => invoke<void>("recovery_start", { name }),
  recoveryComplete: (name: string, code: string, deviceName: string) =>
    invoke<string[]>("recovery_complete", { name, code, deviceName }),
  restoreIdentity: (phrase: string, name: string, deviceName: string) =>
    invoke<string>("restore_identity", { phrase, name, deviceName }),
  myInvite: () => invoke<Invite>("my_invite"),
  addContact: (code: string) => invoke<Contact>("add_contact", { code }),
  addContactByName: (name: string) => invoke<Contact>("add_contact_by_name", { name }),
  setBackupPassword: (password: string) => invoke<void>("set_backup_password", { password }),
  backupNow: () => invoke<void>("backup_now"),
  disableBackup: () => invoke<void>("disable_backup"),
  restoreBackup: (password: string) => invoke<number>("restore_backup", { password }),
  ackKeyChange: (contact: number) => invoke<void>("ack_key_change", { contact }),
  deleteAccount: () => invoke<void>("delete_account"),
  registerAccount: (email: string | null, discoverable: boolean) =>
    invoke<void>("register_account", { email, discoverable }),
  setBadge: (count: number, overlay: number[] | null) => invoke<void>("set_badge", { count, overlay }),
  requestAttention: () => invoke<void>("request_attention"),
  setServer: (host: string | null) => invoke<void>("set_server", { host }),
  sendFiles: (contact: number, paths: string[]) => invoke<void>("send_files", { contact, paths }),
  pickAndSendFiles: (contact: number) => invoke<void>("pick_and_send_files", { contact }),
  downloadFile: (file: string) => invoke<void>("download_file", { file }),
  pauseFile: (file: string) => invoke<void>("pause_file", { file }),
  cancelFile: (file: string) => invoke<void>("cancel_file", { file }),
  openFile: (file: string, reveal: boolean) => invoke<void>("open_file", { file, reveal }),
  getPrefs: () => invoke<{ sounds: boolean }>("get_prefs"),
  setEmail: (email: string) => invoke<void>("set_email", { email }),
  verifyEmail: (code: string) => invoke<void>("verify_email", { code }),
  listContacts: () => invoke<Contact[]>("list_contacts"),
  setVerified: (contact: number, verified: boolean) =>
    invoke<void>("set_verified", { contact, verified }),
  listMessages: (contact: number, before?: number) =>
    invoke<Message[]>("list_messages", { contact, before: before ?? null }),
  sendMessage: (contact: number, text: string) =>
    invoke<Message>("send_message", { contact, text }),
  markRead: (contact: number) => invoke<number>("mark_read", { contact }),
  getSettings: () => invoke<Settings>("get_settings"),
  saveSettings: (settings: Settings) => invoke<void>("save_settings", { settings }),
  onEvent: (cb: (e: OwEvent) => void): Promise<UnlistenFn> =>
    listen<OwEvent>("ow-event", (e) => cb(e.payload)),
};

/** Error codes returned by the backend that have a translated message. */
const KNOWN_ERRORS = [
  "name_taken",
  "user_not_found",
  "own_account",
  "invalid_code",
  "name_mismatch",
  "wrong_password",
  "password_too_short",
  "no_backup",
  "invalid_host",
] as const;

/** Error message from a rejected command (translated when known). */
export function errorText(
  e: unknown,
  tr?: (key: `err_${(typeof KNOWN_ERRORS)[number]}`) => string,
): string {
  const text = typeof e === "string" ? e : e instanceof Error ? e.message : String(e);
  const known = KNOWN_ERRORS.find((k) => k === text);
  return known && tr ? tr(`err_${known}`) : text;
}
