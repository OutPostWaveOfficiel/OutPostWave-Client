// Typed wrappers around the Tauri commands and events of the Rust core.
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export interface AppInfo {
  version: string;
  os: string;
  arch: string;
  profile: string;
}

export interface Contact {
  id: number;
  handle: string;
  displayName: string | null;
  verified: boolean;
  online: boolean;
  safetyNumber: string;
}

export type MessageStatus = "pending" | "sent" | "delivered" | "read" | "received";

export interface Message {
  id: number;
  msgId: string;
  outgoing: boolean;
  sentAt: number;
  status: MessageStatus;
  body: string;
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
  | { type: "resync" };

export const api = {
  appInfo: () => invoke<AppInfo>("app_info"),
  identityStatus: () => invoke<string | null>("identity_status"),
  createIdentity: (name: string, deviceName: string) =>
    invoke<string[]>("create_identity", { name, deviceName }),
  restoreIdentity: (phrase: string, name: string, deviceName: string) =>
    invoke<string>("restore_identity", { phrase, name, deviceName }),
  myInvite: () => invoke<Invite>("my_invite"),
  addContact: (code: string) => invoke<Contact>("add_contact", { code }),
  listContacts: () => invoke<Contact[]>("list_contacts"),
  setVerified: (contact: number, verified: boolean) =>
    invoke<void>("set_verified", { contact, verified }),
  listMessages: (contact: number, before?: number) =>
    invoke<Message[]>("list_messages", { contact, before: before ?? null }),
  sendMessage: (contact: number, text: string) =>
    invoke<Message>("send_message", { contact, text }),
  onEvent: (cb: (e: OwEvent) => void): Promise<UnlistenFn> =>
    listen<OwEvent>("ow-event", (e) => cb(e.payload)),
};

/** Error message from a rejected command. */
export function errorText(e: unknown): string {
  return typeof e === "string" ? e : e instanceof Error ? e.message : String(e);
}
