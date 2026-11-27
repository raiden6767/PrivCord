//! The engine, hosted for the Windows desktop shell (milestone M7).
//!
//! The desktop UI is a web page; the engine is Rust in the Tauri shell. This crate is the seam between them and has no
//! Tauri code, so it builds and is tested on any machine: the shell gives it the platform ports (secure storage, a way to
//! send frames to the page, a notifier) and the page talks to it in JSON:
//!
//! * [`DesktopHost::invoke`] runs one named command (`"send_message"`, `"link_up"`, `"frame_received"`, `"snapshot"`, ...)
//!   with a JSON object of arguments and returns JSON.
//! * [`DesktopHost::next_event`] waits for the next engine event and returns it as JSON.
//!
//! It reuses `privcord-ffi`'s `PrivcordEngine`, so the desktop and Android apps run exactly the same engine surface.
//! Commands are added here as the page moves onto the engine; an unknown command is an error, never a silent no-op.

pub mod autobackup;
pub mod lan;
pub mod native_signaling;
pub mod sealed_file;
pub mod update_guard;

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use privcord_ffi::{
    LinkKind, NotifierCallbacks, PathKind, PrivcordEngine, SecureStoreCallbacks, SignalKindDto,
    TransportCallbacks,
};
use serde_json::{json, Value};
use std::sync::{Arc, OnceLock, Weak};

/// What went wrong, as a short stable string the page can show or branch on.
pub type HostError = String;

pub struct DesktopHost {
    engine: Arc<PrivcordEngine>,
    /// Connections to the account's other devices on the local network, once started (`start_lan`).
    lan: Arc<OnceLock<lan::Lan>>,
}

/// The shell's transport, with frames for a device we reach on the local network sent there instead.
struct LanRouted {
    inner: Arc<dyn TransportCallbacks>,
    lan: Arc<OnceLock<lan::Lan>>,
}

impl TransportCallbacks for LanRouted {
    fn send(&self, peer: String, frame: String) -> bool {
        if self.lan.get().is_some_and(|l| l.send(&peer, &frame)) {
            return true;
        }
        self.inner.send(peer, frame)
    }
    fn send_raw(&self, link: String, frame: String) -> bool {
        self.inner.send_raw(link, frame)
    }
    fn close(&self, peer: String) {
        if let Some(l) = self.lan.get() {
            l.close(&peer);
        }
        self.inner.close(peer);
    }
    fn signal_available(&self) -> bool {
        self.inner.signal_available()
    }
    fn signal_subscribe(&self, mailbox: String) -> bool {
        self.inner.signal_subscribe(mailbox)
    }
    fn signal_unsubscribe(&self, mailbox: String) {
        self.inner.signal_unsubscribe(mailbox);
    }
    fn signal_put(
        &self,
        mailbox: String,
        blob: String,
        kind: privcord_ffi::SignalKind,
        ttl_sec: u32,
    ) -> bool {
        self.inner.signal_put(mailbox, blob, kind, ttl_sec)
    }
}

/// The engine, as the local-network side sees it (weak: the engine owns the transport that owns the network side).
struct EngineLan(Weak<PrivcordEngine>);

impl lan::LanEngine for EngineLan {
    fn beacon(&self, port: u16) -> Option<String> {
        self.0.upgrade()?.lan_beacon(port).ok().flatten()
    }
    fn seen(&self, beacon: &str) -> Option<(String, u16, bool)> {
        let p = self
            .0
            .upgrade()?
            .lan_beacon_seen(beacon.to_owned())
            .ok()??;
        Some((p.address, p.port, p.initiator))
    }
    fn link_up(&self, peer: &str) {
        if let Some(e) = self.0.upgrade() {
            // The engine authenticates and encrypts the link itself, with both devices' own keys.
            let _ = e.link_up(peer.to_owned(), PathKind::DirectLan, LinkKind::Open);
        }
    }
    fn frame(&self, peer: &str, text: &str) {
        if let Some(e) = self.0.upgrade() {
            let _ = e.frame_received(peer.to_owned(), text.to_owned());
        }
    }
    fn link_down(&self, peer: &str) {
        if let Some(e) = self.0.upgrade() {
            let _ = e.link_down(peer.to_owned());
        }
    }
}

impl std::fmt::Debug for DesktopHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("DesktopHost(..)")
    }
}

fn text(args: &Value, key: &str) -> Result<String, HostError> {
    args.get(key)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("missing text argument `{key}`"))
}

fn maybe_text(args: &Value, key: &str) -> Option<String> {
    args.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn number(args: &Value, key: &str) -> Result<u32, HostError> {
    args.get(key)
        .and_then(Value::as_u64)
        .and_then(|n| u32::try_from(n).ok())
        .ok_or_else(|| format!("missing number argument `{key}`"))
}

fn big(args: &Value, key: &str) -> Result<i64, HostError> {
    args.get(key)
        .and_then(Value::as_i64)
        .ok_or_else(|| format!("missing number argument `{key}`"))
}

fn minutes(args: &Value, key: &str) -> Result<u16, HostError> {
    u16::try_from(number(args, key)?).map_err(|_| format!("missing number argument `{key}`"))
}

fn texts(args: &Value, key: &str) -> Result<Vec<String>, HostError> {
    args.get(key)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|v| v.as_str().map(str::to_owned))
                .collect()
        })
        .ok_or_else(|| format!("missing list argument `{key}`"))
}

/// File bytes cross the page boundary as base64 text.
fn bytes(args: &Value, key: &str) -> Result<Vec<u8>, HostError> {
    B64.decode(text(args, key)?)
        .map_err(|_| format!("`{key}` is not valid base64"))
}

fn flag(args: &Value, key: &str) -> Result<bool, HostError> {
    args.get(key)
        .and_then(Value::as_bool)
        .ok_or_else(|| format!("missing true/false argument `{key}`"))
}

fn path_kind(name: &str) -> Result<PathKind, HostError> {
    match name {
        "direct_lan" => Ok(PathKind::DirectLan),
        "direct_nat" => Ok(PathKind::DirectNat),
        "relayed" => Ok(PathKind::Relayed),
        "bluetooth" => Ok(PathKind::Bluetooth),
        "unknown" => Ok(PathKind::Unknown),
        other => Err(format!("unknown path `{other}`")),
    }
}

fn link_kind(name: &str) -> Result<LinkKind, HostError> {
    match name {
        // A WebRTC data channel is already encrypted; the engine speaks its peer handshake over it.
        "encrypted" => Ok(LinkKind::Encrypted),
        "open" => Ok(LinkKind::Open),
        // Bluetooth, the way the other apps run it.
        "layered" => Ok(LinkKind::Layered),
        other => Err(format!("unknown link kind `{other}`")),
    }
}

fn signal_kind(name: &str) -> Result<SignalKindDto, HostError> {
    match name {
        "ready" => Ok(SignalKindDto::Ready),
        "offer" => Ok(SignalKindDto::Offer),
        "answer" => Ok(SignalKindDto::Answer),
        other => Err(format!("unknown signal `{other}`")),
    }
}

fn json_of<T: serde::Serialize>(value: &T) -> Result<Value, HostError> {
    serde_json::to_value(value).map_err(|e| e.to_string())
}

fn unit<E: std::fmt::Debug>(result: Result<(), E>) -> Result<Value, HostError> {
    result.map(|()| Value::Null).map_err(failed)
}

fn failed<E: std::fmt::Debug>(e: E) -> HostError {
    format!("{e:?}")
}

impl DesktopHost {
    /// Start the engine on the database at `db_path`. `context` is the app's conversation id ("duo" unless the build
    /// sets another); it must match the other apps'.
    pub fn start(
        db_path: &str,
        context: &str,
        secure: Arc<dyn SecureStoreCallbacks>,
        transport: Arc<dyn TransportCallbacks>,
        notifier: Arc<dyn NotifierCallbacks>,
    ) -> Result<Self, HostError> {
        let lan = Arc::new(OnceLock::new());
        let transport = Arc::new(LanRouted {
            inner: transport,
            lan: lan.clone(),
        });
        let engine = PrivcordEngine::start(
            db_path.to_owned(),
            context.to_owned(),
            secure,
            transport,
            notifier,
        )
        .map_err(failed)?;
        Ok(Self { engine, lan })
    }

    /// Find the account's other devices on the local network and link to them directly (no internet needed). Once.
    pub fn start_lan(&self, config: lan::LanConfig) -> Result<(), HostError> {
        if self.lan.get().is_some() {
            return Ok(());
        }
        let started = lan::Lan::start(config, Arc::new(EngineLan(Arc::downgrade(&self.engine))))
            .map_err(failed)?;
        let _ = self.lan.set(started);
        Ok(())
    }

    /// Run one command. Arguments are a JSON object; the result is JSON (`null` when there is nothing to return).
    /// Sends a file whose bytes arrive as they are (the page hands them over raw, not as base64 text inside JSON, which
    /// for a 50 MB file would be a 67 MB string). `args` carries `peer` or `group`, `name`, `mime` and maybe `reply_to`.
    pub fn send_file_bytes(&self, args: &Value, data: Vec<u8>) -> Result<Value, HostError> {
        let e = &self.engine;
        let id = if let Some(group) = maybe_text(args, "group") {
            e.send_group_file(
                group,
                text(args, "name")?,
                text(args, "mime")?,
                data,
                maybe_text(args, "reply_to"),
            )
        } else {
            e.send_file(
                text(args, "peer")?,
                text(args, "name")?,
                text(args, "mime")?,
                data,
                maybe_text(args, "reply_to"),
            )
        }
        .map_err(failed)?;
        Ok(json!(id))
    }

    /// The bytes of a stored file as they are, or `None` if it is no longer stored.
    pub fn load_file_bytes(&self, message_id: &str) -> Result<Option<Vec<u8>>, HostError> {
        Ok(self
            .engine
            .load_file(message_id.to_owned())
            .map_err(failed)?
            .map(|f| f.data))
    }

    pub fn invoke(&self, command: &str, args: &Value) -> Result<Value, HostError> {
        let e = &self.engine;
        match command {
            // identity
            "create_identity" => Ok(json!(e.create_identity().map_err(failed)?)),
            "recover_identity" => {
                e.recover_identity(
                    text(args, "phrase")?,
                    maybe_text(args, "passphrase").unwrap_or_default(),
                )
                .map_err(failed)?;
                Ok(Value::Null)
            }
            "own_card" => Ok(json!(e
                .own_card(maybe_text(args, "name"))
                .map_err(failed)?)),
            "set_display_name" => {
                e.set_display_name(text(args, "name")?).map_err(failed)?;
                Ok(Value::Null)
            }
            // contacts and invites
            "add_contact" => Ok(json!(e
                .add_contact(text(args, "card")?, flag(args, "verified")?)
                .map_err(failed)?)),
            "create_invite" => json_of(&e.create_invite().map_err(failed)?),
            "preview_invite" => json_of(&e.preview_invite(text(args, "text")?).map_err(failed)?),
            "accept_invite" => Ok(json!(e
                .accept_invite(text(args, "text")?)
                .map_err(failed)?)),
            // messages
            "send_message" => Ok(json!(e
                .send_message(
                    text(args, "peer")?,
                    text(args, "text")?,
                    maybe_text(args, "reply_to")
                )
                .map_err(failed)?)),
            "mark_read" => {
                e.mark_read(text(args, "peer")?).map_err(failed)?;
                Ok(Value::Null)
            }
            // links: the page tells the engine when its WebRTC or Bluetooth link opens, closes, or receives a frame
            "link_up" => {
                e.link_up(
                    text(args, "peer")?,
                    path_kind(&text(args, "path")?)?,
                    link_kind(&text(args, "kind")?)?,
                )
                .map_err(failed)?;
                Ok(Value::Null)
            }
            "link_down" => {
                e.link_down(text(args, "peer")?).map_err(failed)?;
                Ok(Value::Null)
            }
            "frame_received" => {
                e.frame_received(text(args, "peer")?, text(args, "frame")?)
                    .map_err(failed)?;
                Ok(Value::Null)
            }
            "reset_dial_backoff" => {
                e.reset_dial_backoff(maybe_text(args, "peer"))
                    .map_err(failed)?;
                Ok(Value::Null)
            }
            // signaling: blobs that arrive from a relay, and connection setup to a friend
            "signal_blob" => {
                e.signal_blob(text(args, "mailbox")?, text(args, "blob")?)
                    .map_err(failed)?;
                Ok(Value::Null)
            }
            "send_signal" => {
                e.send_signal(
                    text(args, "peer")?,
                    signal_kind(&text(args, "kind")?)?,
                    maybe_text(args, "sdp"),
                )
                .map_err(failed)?;
                Ok(Value::Null)
            }
            // groups
            "create_group" => Ok(json!(e
                .create_group(text(args, "name")?, texts(args, "members")?)
                .map_err(failed)?)),
            "add_group_member" => {
                unit(e.add_group_member(text(args, "group")?, text(args, "uid")?))
            }
            "remove_group_member" => {
                unit(e.remove_group_member(text(args, "group")?, text(args, "uid")?))
            }
            "leave_group" => unit(e.leave_group(text(args, "group")?)),
            "recreate_legacy_group" => Ok(json!(e
                .recreate_legacy_group(text(args, "old_id")?)
                .map_err(failed)?)),
            "dismiss_legacy_group" => unit(e.dismiss_legacy_group(text(args, "old_id")?)),
            "rename_group" => unit(e.rename_group(text(args, "group")?, text(args, "name")?)),
            "send_group_message" => Ok(json!(e
                .send_group_message(
                    text(args, "group")?,
                    text(args, "text")?,
                    maybe_text(args, "reply_to")
                )
                .map_err(failed)?)),
            // files (bytes are base64)
            "send_file" => Ok(json!(e
                .send_file(
                    text(args, "peer")?,
                    text(args, "name")?,
                    text(args, "mime")?,
                    bytes(args, "data")?,
                    maybe_text(args, "reply_to")
                )
                .map_err(failed)?)),
            "send_group_file" => Ok(json!(e
                .send_group_file(
                    text(args, "group")?,
                    text(args, "name")?,
                    text(args, "mime")?,
                    bytes(args, "data")?,
                    maybe_text(args, "reply_to")
                )
                .map_err(failed)?)),
            "load_file" => Ok(
                match e.load_file(text(args, "message_id")?).map_err(failed)? {
                    Some(f) => {
                        json!({ "name": f.name, "mime": f.mime, "data": B64.encode(f.data) })
                    }
                    None => Value::Null,
                },
            ),
            // public channels
            "join_channel" => Ok(json!(e
                .join_channel(text(args, "name")?)
                .map_err(failed)?)),
            "leave_channel" => unit(e.leave_channel(text(args, "name")?)),
            "send_channel_message" => Ok(json!(e
                .send_channel_message(text(args, "name")?, text(args, "text")?)
                .map_err(failed)?)),
            // contacts and people
            "set_checked" => unit(e.set_checked(text(args, "uid")?, flag(args, "checked")?)),
            "rename_contact" => unit(e.rename_contact(text(args, "uid")?, text(args, "name")?)),
            "remove_contact" => unit(e.remove_contact(text(args, "uid")?)),
            "set_muted" => unit(e.set_muted(text(args, "uid")?, flag(args, "muted")?)),
            "toggle_pin" => unit(e.toggle_pin(text(args, "conversation")?)),
            "set_archived" => {
                unit(e.set_archived(text(args, "conversation")?, flag(args, "archived")?))
            }
            "set_silent" => unit(e.set_silent(text(args, "conversation")?, flag(args, "silent")?)),
            "set_mute_until" => {
                unit(e.set_mute_until(text(args, "conversation")?, big(args, "until_ms")?))
            }
            "set_quiet_hours" => unit(e.set_quiet_hours(
                flag(args, "enabled")?,
                minutes(args, "from")?,
                minutes(args, "to")?,
            )),
            "prefs" => {
                Ok(serde_json::from_str(&e.prefs_json().map_err(failed)?).unwrap_or(Value::Null))
            }
            "message_receipts" => Ok(serde_json::from_str(
                &e.message_receipts_json(text(args, "message_id")?)
                    .map_err(failed)?,
            )
            .unwrap_or(Value::Null)),
            "mesh_hops" => {
                Ok(serde_json::from_str(&e.mesh_hops_json().map_err(failed)?)
                    .unwrap_or(Value::Null))
            }
            "set_secure_screen" => unit(e.set_secure_screen(flag(args, "on")?)),
            "diagnostics" => Ok(json!(e
                .diagnostics_json(text(args, "app")?)
                .map_err(failed)?)),
            "diag_log" => {
                e.diag_log(
                    text(args, "category")?,
                    text(args, "event")?,
                    text(args, "detail")?,
                );
                Ok(Value::Null)
            }
            "diag_export" => Ok(json!(e
                .diag_export(text(args, "about")?)
                .map_err(failed)?)),
            "diag_clear" => unit(e.diag_clear()),
            "set_diag_enabled" => unit(
                e.set_diag_enabled(args.get("enabled").and_then(Value::as_bool).unwrap_or(true)),
            ),
            "diag_status" => {
                Ok(serde_json::from_str(&e.diag_status_json().map_err(failed)?)
                    .unwrap_or(Value::Null))
            }
            "self_test" => {
                Ok(serde_json::from_str(&e.self_test_json().map_err(failed)?)
                    .unwrap_or(Value::Null))
            }
            "storage_by_chat" => Ok(serde_json::from_str(
                &e.storage_by_chat_json().map_err(failed)?,
            )
            .unwrap_or(Value::Null)),
            "insight" => {
                let out = e
                    .insight(text(args, "name")?, text(args, "input")?)
                    .map_err(failed)?;
                Ok(serde_json::from_str(&out).unwrap_or(Value::Null))
            }
            "quick_times" => Ok(
                serde_json::from_str(&e.quick_send_times_json().map_err(failed)?)
                    .unwrap_or(Value::Null),
            ),
            "set_utc_offset" => unit(
                e.set_utc_offset_minutes(
                    args.get("minutes")
                        .and_then(Value::as_i64)
                        .and_then(|m| i32::try_from(m).ok())
                        .ok_or_else(|| "missing number argument `minutes`".to_owned())?,
                ),
            ),
            "should_notify" => Ok(json!(e
                .should_notify(text(args, "conversation")?)
                .map_err(failed)?)),
            "set_group_only" => unit(e.set_group_only(flag(args, "value")?)),
            "set_store_forward" => unit(e.set_store_forward(flag(args, "value")?)),
            "accept_request" => Ok(json!(e
                .accept_request(text(args, "uid")?)
                .map_err(failed)?)),
            "dismiss_request" => unit(e.dismiss_request(text(args, "uid")?)),
            "stop_invite" => unit(e.stop_invite()),
            "retry_answer" => unit(e.retry_answer()),
            "clear_answer" => unit(e.clear_answer()),
            // message actions
            "delete_message" => {
                unit(e.delete_message(text(args, "message_id")?, flag(args, "everyone")?))
            }
            "react" => unit(e.react(text(args, "message_id")?, text(args, "emoji")?)),
            "retry_message" => unit(e.retry_message(text(args, "message_id")?)),
            "mark_conversation_read" => unit(e.mark_conversation_read(text(args, "conversation")?)),
            "search" => json_of(
                &e.search(text(args, "query")?, number(args, "limit").unwrap_or(50))
                    .map_err(failed)?,
            ),
            // Bluetooth: the secret shared with a friend (hex), from which the page works out the tags to scan for
            "ble_secret" => Ok(json!(e
                .ble_secret(text(args, "peer")?)
                .map_err(failed)?
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>())),
            // link bookkeeping
            "set_link_path" => {
                unit(e.set_link_path(text(args, "peer")?, path_kind(&text(args, "path")?)?))
            }
            // backup and wiping
            "start_device_link" => Ok(json!(e.start_device_link().map_err(failed)?)),
            "join_device_link" => {
                unit(e.join_device_link(text(args, "code")?, text(args, "name")?))
            }
            "start_scan_link_offer" => Ok(json!(e
                .start_scan_link_offer(text(args, "name")?)
                .map_err(failed)?)),
            "host_scanned_link" => unit(e.host_scanned_link(text(args, "code")?)),
            "submit_link_code" => Ok(json!(e
                .submit_link_code(text(args, "digits")?)
                .map_err(failed)?)),
            "confirm_device_link" => unit(
                e.confirm_device_link(args.get("accept").and_then(Value::as_bool).unwrap_or(false)),
            ),
            "cancel_device_link" => unit(e.cancel_device_link()),
            "request_device_removal" => unit(e.request_device_removal(text(args, "device")?)),
            "schedule_message" => unit(e.schedule_message(
                text(args, "target")?,
                args.get("group").and_then(Value::as_bool).unwrap_or(false),
                text(args, "text")?,
                args.get("at").and_then(Value::as_i64).unwrap_or(0),
            )),
            "cancel_scheduled" => unit(e.cancel_scheduled(text(args, "id")?)),
            "cancel_device_removal" => unit(e.cancel_device_removal(text(args, "device")?)),
            "remove_device_now" => unit(
                e.remove_device_now(
                    text(args, "device")?,
                    text(args, "phrase")?,
                    args.get("passphrase")
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .to_owned(),
                ),
            ),
            "set_bulk_allowed" => unit(
                e.set_bulk_allowed(args.get("allowed").and_then(Value::as_bool).unwrap_or(true)),
            ),
            "export_backup" => Ok(json!(e
                .export_backup(text(args, "passphrase")?)
                .map_err(failed)?)),
            "import_backup" => json_of(
                &e.import_backup(text(args, "text")?, text(args, "passphrase")?)
                    .map_err(failed)?,
            ),
            "wipe_conversation" => json_of(
                &e.wipe_conversation(
                    text(args, "conversation")?,
                    flag(args, "erase_session").unwrap_or(false),
                )
                .map_err(failed)?,
            ),
            "wipe_everything" => json_of(&e.wipe_everything().map_err(failed)?),
            // everything the screens show
            "snapshot" => json_of(&e.snapshot().map_err(failed)?),
            // the same with only the newest messages of each chat, and the older ones a page at a time
            "snapshot_recent" => json_of(
                &e.snapshot_recent(number(args, "per_chat")?)
                    .map_err(failed)?,
            ),
            "messages_before" => json_of(
                &e.messages_before(
                    text(args, "conversation")?,
                    maybe_text(args, "before"),
                    number(args, "limit")?,
                )
                .map_err(failed)?,
            ),
            other => Err(format!("unknown command `{other}`")),
        }
    }

    /// Wait up to `timeout_ms` for the next engine event; `None` if none came. Events are serde's default JSON for the
    /// FFI event type: `{"MessageAdded": {"conversation": ..., "message_id": ..., "own": false}}`.
    pub fn next_event(&self, timeout_ms: u32) -> Option<Value> {
        self.engine
            .next_event(timeout_ms)
            .and_then(|event| serde_json::to_value(event).ok())
    }

    /// Stop the engine and wait for it to finish. Safe to call more than once.
    pub fn shutdown(&self) {
        if let Some(l) = self.lan.get() {
            l.stop();
        }
        let _ = self.engine.shutdown();
    }
}
