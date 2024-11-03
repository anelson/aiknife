
// This file was generated by [tauri-specta](https://github.com/oscartbeaumont/tauri-specta). Do not edit this file manually.

/** user-defined commands **/


export const commands = {
async newSession() : Promise<SessionHandle> {
    return await TAURI_INVOKE("new_session");
},
async sendMessage(session: SessionHandle, message: string) : Promise<string> {
    return await TAURI_INVOKE("send_message", { session, message });
},
async checkApiKey() : Promise<null> {
    return await TAURI_INVOKE("check_api_key");
}
}

/** user-defined events **/


export const events = __makeEvents__<{
messageError: MessageError,
messagePending: MessagePending,
messageResponse: MessageResponse
}>({
messageError: "message-error",
messagePending: "message-pending",
messageResponse: "message-response"
})

/** user-defined constants **/



/** user-defined types **/

export type ChatMessage = { id: string; role: Role; content: string; status: MessageStatus }
export type MessageError = { message_id: string; error: UiError }
export type MessagePending = { message: ChatMessage }
export type MessageResponse = { message: ChatMessage }
export type MessageStatus = "Pending" | "Complete"
export type Role = "user" | "assistant"
export type SessionHandle = { id: string }
/**
 * Fake struct that matches the shape of the JSON serialized form of [`UiError`].
 * 
 * Used to generate the `specta::Type` implementation that describes what UiError actually looks
 * like when serialized.  The `specta` crate is definitely *not* intended for hand-rolled impls of
 * `Type`, so this is just cleaner and simpler
 */
export type UiError = { type: UiErrorType; message: string; details: string | null }
/**
 * Auto-generated discriminant enum variants
 */
export type UiErrorType = "Application" | 
/**
 * A tauri::Error, but wrapped in anyhow::Error because we need that representation to
 * serialize
 */
"Tauri" | "AnotherError"

/** tauri-specta globals **/

import {
	invoke as TAURI_INVOKE,
	Channel as TAURI_CHANNEL,
} from "@tauri-apps/api/core";
import * as TAURI_API_EVENT from "@tauri-apps/api/event";
import { type WebviewWindow as __WebviewWindow__ } from "@tauri-apps/api/webviewWindow";

type __EventObj__<T> = {
	listen: (
		cb: TAURI_API_EVENT.EventCallback<T>,
	) => ReturnType<typeof TAURI_API_EVENT.listen<T>>;
	once: (
		cb: TAURI_API_EVENT.EventCallback<T>,
	) => ReturnType<typeof TAURI_API_EVENT.once<T>>;
	emit: null extends T
		? (payload?: T) => ReturnType<typeof TAURI_API_EVENT.emit>
		: (payload: T) => ReturnType<typeof TAURI_API_EVENT.emit>;
};

export type Result<T, E> =
	| { status: "ok"; data: T }
	| { status: "error"; error: E };

function __makeEvents__<T extends Record<string, any>>(
	mappings: Record<keyof T, string>,
) {
	return new Proxy(
		{} as unknown as {
			[K in keyof T]: __EventObj__<T[K]> & {
				(handle: __WebviewWindow__): __EventObj__<T[K]>;
			};
		},
		{
			get: (_, event) => {
				const name = mappings[event as keyof T];

				return new Proxy((() => {}) as any, {
					apply: (_, __, [window]: [__WebviewWindow__]) => ({
						listen: (arg: any) => window.listen(name, arg),
						once: (arg: any) => window.once(name, arg),
						emit: (arg: any) => window.emit(name, arg),
					}),
					get: (_, command: keyof __EventObj__<any>) => {
						switch (command) {
							case "listen":
								return (arg: any) => TAURI_API_EVENT.listen(name, arg);
							case "once":
								return (arg: any) => TAURI_API_EVENT.once(name, arg);
							case "emit":
								return (arg: any) => TAURI_API_EVENT.emit(name, arg);
						}
					},
				});
			},
		},
	);
}
