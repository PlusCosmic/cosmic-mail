// Wire types — derived from the bindings that `wails3 generate bindings`
// writes from internal/models/models.go. Nothing here is hand-maintained
// beyond the mapping: regenerate rather than edit. Shapes are bound by
// docs/ARCHITECTURE.md ("Wire types").
//
// The generator types every Go slice as `T[] | null` because a nil slice
// would serialise as null. The backend never sends one (see models.NonNil),
// so the list fields are narrowed back to plain arrays here.

import type * as m from "$bindings/cosmicmail/internal/models/models";

type Lists<T> = { [K in keyof T]: T[K] extends (infer U)[] | null ? U[] : T[K] };

/** A generated enum's string values, minus the Go zero value it also declares. */
type EnumValues<E extends string> = Exclude<`${E}`, "">;

export type AccountKind = EnumValues<m.AccountKind>;

export interface Account extends Omit<m.Account, "kind"> {
	kind: AccountKind;
}

export type FolderRole =
	| "inbox"
	| "sent"
	| "drafts"
	| "trash"
	| "archive"
	| "spam"
	| "normal";

export interface Folder extends Omit<m.Folder, "role"> {
	role: FolderRole;
}

export type MessageSummary = m.MessageSummary;
export type AttachmentInfo = m.AttachmentInfo;
export type MessageBody = Lists<m.MessageBody>;

export type ShipmentCarrier = "ups" | "fedex" | "usps" | "dhl" | "royal_mail" | "amazon";

export interface Shipment extends Omit<m.Shipment, "carrier"> {
	carrier: ShipmentCarrier; // stable code; see carrierLabel()/carrierGlyph() in format.ts
}

export type OmarchyTheme = Lists<m.OmarchyTheme>;

export type SyncState = EnumValues<m.SyncState>;

export type DiscoverySource = EnumValues<m.DiscoverySource>;

export interface DiscoveredConfig extends Omit<m.DiscoveredConfig, "kind" | "source"> {
	kind: AccountKind; // "gmail" ⇒ frontend steers user to the Gmail OAuth tab
	source: DiscoverySource;
}

export type ImapAccountInput = m.ImapAccountInput;
export type SendMessageInput = Lists<m.SendMessageInput>;
export type Settings = m.Settings;

// ---- Event payloads (backend -> frontend) ----

export type NewMessagesEvent = Lists<m.NewMessagesEvent>;
export type MessagesUpdatedEvent = m.MessagesUpdatedEvent;

export interface SyncStateEvent extends Omit<m.SyncStateEvent, "state"> {
	state: SyncState;
}
