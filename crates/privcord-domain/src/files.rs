//! File sharing rules (`files.ts`): sizes, names, chunking, offers and the receiving assembler.

use crate::{
    codec,
    error::DomainError,
    ids::MessageId,
    limits::{
        FILE_CHUNK_BYTES, MAX_FILE_BYTES, MAX_FILE_CHUNKS, MAX_FILE_NAME, MAX_INCOMING_PER_PEER,
        MAX_REPLY_PREVIEW, PROTOCOL_VERSION,
    },
    message::{parse_timestamp, DeclineReason, ReplyRef},
    text::{is_js_whitespace, take_code_points, utf16_len},
};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};
use std::collections::HashMap;

const INLINE_IMAGE: [&str; 4] = ["image/png", "image/jpeg", "image/gif", "image/webp"];
const INLINE_VIDEO: [&str; 4] = ["video/mp4", "video/webm", "video/quicktime", "video/ogg"];
const INLINE_AUDIO: [&str; 7] = [
    "audio/webm",
    "audio/ogg",
    "audio/mp4",
    "audio/mpeg",
    "audio/aac",
    "audio/wav",
    "audio/x-m4a",
];

/// Types that may be previewed in place. SVG is never one of them (it can carry script).
pub fn is_inline_image(mime: &str) -> bool {
    INLINE_IMAGE.contains(&mime.to_lowercase().as_str())
}
pub fn is_inline_video(mime: &str) -> bool {
    INLINE_VIDEO.contains(&mime.to_lowercase().as_str())
}
pub fn is_inline_audio(mime: &str) -> bool {
    let lower = mime.to_lowercase();
    let base = lower
        .split(';')
        .next()
        .unwrap_or("")
        .trim_matches(is_js_whitespace);
    INLINE_AUDIO.contains(&base)
}

/// A safe file name: path and control characters become `_`, leading dots and spaces go, at most 120 code points
/// (a short extension is kept), never empty.
pub fn sanitize_file_name(raw: &str) -> String {
    let replaced: String = raw
        .chars()
        .map(|c| {
            // Bidi controls and zero-width characters are replaced too: "invoice\u{202e}txt.exe" would otherwise display reversed.
            if c <= '\u{1f}'
                || c == '\u{7f}'
                || "\\/:*?\"<>|".contains(c)
                || ('\u{200b}'..='\u{200f}').contains(&c)
                || ('\u{202a}'..='\u{202e}').contains(&c)
                || ('\u{2066}'..='\u{2069}').contains(&c)
            {
                '_'
            } else {
                c
            }
        })
        .collect();
    let stripped = replaced.trim_start_matches(|c: char| c == '.' || is_js_whitespace(c));
    let name = crate::text::js_trim(stripped).to_owned();
    let chars: Vec<char> = name.chars().collect();
    let name = if chars.len() > MAX_FILE_NAME {
        let ext = match name.rfind('.') {
            Some(byte_at) if byte_at > 0 => {
                let ext = &name[byte_at..];
                if utf16_len(ext) <= 12 {
                    ext
                } else {
                    ""
                }
            }
            _ => "",
        };
        let keep = MAX_FILE_NAME.saturating_sub(ext.chars().count());
        let mut s: String = chars.iter().take(keep).collect();
        s.push_str(ext);
        s
    } else {
        name
    };
    if name.is_empty() {
        "file".to_owned()
    } else {
        name
    }
}

/// `type/subtype` made of safe characters, lower-cased; anything else becomes `application/octet-stream`.
pub fn sanitize_mime(raw: &str) -> String {
    fn ok_first(c: char) -> bool {
        c.is_ascii_alphanumeric()
    }
    fn ok_rest(c: char) -> bool {
        c.is_ascii_alphanumeric() || "!#$&^_.+-".contains(c)
    }
    let valid = (|| {
        let (ty, sub) = raw.split_once('/')?;
        let part = |s: &str, max: usize| {
            let mut it = s.chars();
            let first = it.next().is_some_and(ok_first);
            first && s.chars().skip(1).all(ok_rest) && s.chars().count() <= max + 1
        };
        // The pattern allows 1 + up to 60 characters before the slash and 1 + up to 100 after it.
        (part(ty, 60) && part(sub, 100) && !sub.contains('/')).then_some(())
    })();
    if valid.is_some() {
        raw.to_lowercase()
    } else {
        "application/octet-stream".to_owned()
    }
}

pub fn validate_file_size(size: usize) -> Result<(), DomainError> {
    if size == 0 {
        return Err(DomainError::EmptyFile);
    }
    if size > MAX_FILE_BYTES {
        return Err(DomainError::FileTooLarge {
            limit: format_bytes(MAX_FILE_BYTES),
        });
    }
    Ok(())
}

pub fn format_bytes(n: usize) -> String {
    if n < 1024 {
        format!("{n} B")
    } else if n < 1024 * 1024 {
        let k = n as f64 / 1024.0;
        if n < 10 * 1024 {
            format!("{k:.1} KB")
        } else {
            format!("{k:.0} KB")
        }
    } else {
        format!("{:.1} MB", n as f64 / (1024.0 * 1024.0))
    }
}

/// Text shown when a file is quoted in a reply or listed as a last message.
pub fn file_preview_text(name: &str) -> String {
    take_code_points(&format!("📎 {name}"), MAX_REPLY_PREVIEW)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileOffer {
    pub message_id: MessageId,
    pub group_id: Option<String>,
    pub name: String,
    pub mime: String,
    pub size: usize,
    /// base64url SHA-256 of the whole file.
    pub sha256: String,
    pub client_created_at: i64,
    pub reply_to: Option<ReplyRef>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileChunk {
    pub message_id: MessageId,
    pub index: usize,
    /// base64url of up to 12 KiB.
    pub data: String,
}

pub fn sha256_b64(bytes: &[u8]) -> String {
    codec::encode(&Sha256::digest(bytes))
}

impl FileOffer {
    /// Describe bytes we are about to send. Fails if the file is empty or over the limit.
    pub fn make(
        message_id: MessageId,
        group_id: Option<String>,
        name: &str,
        mime: &str,
        bytes: &[u8],
        client_created_at: i64,
        reply_to: Option<ReplyRef>,
    ) -> Result<Self, DomainError> {
        validate_file_size(bytes.len())?;
        Ok(Self {
            message_id,
            group_id: group_id.filter(|g| !g.is_empty()),
            name: sanitize_file_name(name),
            mime: sanitize_mime(mime),
            size: bytes.len(),
            sha256: sha256_b64(bytes),
            client_created_at,
            reply_to,
        })
    }

    pub fn to_frame(&self) -> Value {
        let mut o = Map::new();
        o.insert("v".into(), json!(PROTOCOL_VERSION));
        o.insert("kind".into(), json!("file_offer"));
        o.insert("messageId".into(), json!(self.message_id));
        if let Some(g) = &self.group_id {
            o.insert("groupId".into(), json!(g));
        }
        o.insert("name".into(), json!(self.name));
        o.insert("mime".into(), json!(self.mime));
        o.insert("size".into(), json!(self.size));
        o.insert("sha256".into(), json!(self.sha256));
        o.insert("clientCreatedAt".into(), json!(self.client_created_at));
        if let Some(r) = &self.reply_to {
            o.insert("replyTo".into(), json!(r));
        }
        Value::Object(o)
    }

    /// `None` for anything that breaks the rules. Name and type are cleaned on the way in.
    pub fn from_frame(value: &Value) -> Option<Self> {
        let o = value.as_object()?;
        if o.get("kind")?.as_str()? != "file_offer" || o.get("v")?.as_u64()? != PROTOCOL_VERSION {
            return None;
        }
        let message_id = MessageId::new(o.get("messageId")?.as_str()?).ok()?;
        let group_id = match o.get("groupId") {
            None => None,
            Some(v) => {
                let g = v.as_str()?;
                if g.is_empty() || utf16_len(g) > 128 {
                    return None;
                }
                Some(g.to_owned())
            }
        };
        let name = o.get("name")?.as_str()?;
        if name.is_empty() || utf16_len(name) > 400 {
            return None;
        }
        let mime = o.get("mime")?.as_str()?;
        if utf16_len(mime) > 200 {
            return None;
        }
        let size = o.get("size")?.as_f64()?;
        if size.fract() != 0.0 || size <= 0.0 || size > 9_007_199_254_740_991.0 {
            return None;
        }
        let sha = o.get("sha256")?.as_str()?;
        let sha_len = utf16_len(sha);
        if !(40..=64).contains(&sha_len) {
            return None;
        }
        let client_created_at = parse_timestamp(o.get("clientCreatedAt")?)?;
        let reply_to = match o.get("replyTo") {
            None => None,
            Some(v) => Some(ReplyRef::from_value(v)?),
        };
        Some(Self {
            message_id,
            group_id,
            name: sanitize_file_name(name),
            mime: sanitize_mime(mime),
            size: size as usize,
            sha256: sha.to_owned(),
            client_created_at,
            reply_to,
        })
    }
}

impl FileChunk {
    pub fn to_frame(&self) -> Value {
        json!({ "v": PROTOCOL_VERSION, "kind": "file_chunk", "messageId": self.message_id, "index": self.index, "data": self.data })
    }

    pub fn from_frame(value: &Value) -> Option<Self> {
        let o = value.as_object()?;
        if o.get("kind")?.as_str()? != "file_chunk" || o.get("v")?.as_u64()? != PROTOCOL_VERSION {
            return None;
        }
        let message_id = MessageId::new(o.get("messageId")?.as_str()?).ok()?;
        let index = o.get("index")?.as_f64()?;
        if index.fract() != 0.0 || index < 0.0 || index >= MAX_FILE_CHUNKS as f64 {
            return None;
        }
        let data = o.get("data")?.as_str()?;
        let max = (FILE_CHUNK_BYTES * 4).div_ceil(3) + 4;
        if data.is_empty() || utf16_len(data) > max {
            return None;
        }
        Some(Self {
            message_id,
            index: index as usize,
            data: data.to_owned(),
        })
    }
}

/// The chunks of a file, in order.
pub fn file_chunks<'a>(
    message_id: &'a MessageId,
    bytes: &'a [u8],
) -> impl Iterator<Item = FileChunk> + 'a {
    bytes
        .chunks(FILE_CHUNK_BYTES)
        .enumerate()
        .map(|(index, part)| FileChunk {
            message_id: message_id.clone(),
            index,
            data: codec::encode(part),
        })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BeginOutcome {
    Ok,
    TooLarge,
    Busy,
}

#[derive(Debug, PartialEq, Eq)]
pub enum AssembleResult {
    More,
    Done {
        offer: Box<FileOffer>,
        bytes: Vec<u8>,
    },
    Failed(DeclineReason),
}

/// Where the bytes of a file being received wait until it is whole. The default keeps them as they are; the engine gives
/// it one that keeps every chunk sealed in memory.
pub trait ChunkStore: Send + std::fmt::Debug {
    /// Adds the next chunk.
    fn push(&mut self, chunk: &[u8]);
    /// Chunks `from..to` (by number) as one buffer.
    fn range(&self, from: usize, to: usize) -> Vec<u8>;
    /// Everything held, in order. The store is empty afterwards.
    fn take(&mut self) -> Vec<u8>;
}

/// The plain store: the bytes in one buffer.
#[derive(Debug, Default)]
pub struct PlainChunks(Vec<u8>);

impl ChunkStore for PlainChunks {
    fn push(&mut self, chunk: &[u8]) {
        self.0.extend_from_slice(chunk);
    }
    fn range(&self, from: usize, to: usize) -> Vec<u8> {
        let (a, b) = (
            from * FILE_CHUNK_BYTES,
            (to * FILE_CHUNK_BYTES).min(self.0.len()),
        );
        self.0.get(a..b).map(<[u8]>::to_vec).unwrap_or_default()
    }
    fn take(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.0)
    }
}

#[derive(Debug)]
struct Incoming {
    offer: FileOffer,
    bytes: Box<dyn ChunkStore>,
    /// Bytes held (whole chunks, except after the last one).
    held: usize,
    next: usize,
}

/// Receiving side, one per peer link. Chunks must arrive in order (the data channel is ordered; one we already hold is
/// skipped); anything out of order, the wrong size, over the file size or failing the SHA-256 check aborts that transfer.
#[derive(Debug, Default)]
pub struct FileAssembler {
    active: HashMap<String, Incoming>,
}

impl FileAssembler {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn active_count(&self) -> usize {
        self.active.len()
    }

    /// Start a transfer, or say why it is refused.
    pub fn begin(&mut self, offer: FileOffer) -> BeginOutcome {
        let key = offer.message_id.to_string();
        // A repeated offer is the sender starting over (its retry timer ran out): the link is ordered, so whatever
        // chunks of the first attempt are still ahead of it have already been applied. Start again from nothing.
        let restart = self.active.remove(&key).is_some();
        if validate_file_size(offer.size).is_err() {
            return BeginOutcome::TooLarge;
        }
        if !restart && self.active.len() >= MAX_INCOMING_PER_PEER {
            return BeginOutcome::Busy;
        }
        // Nothing is reserved for the whole file: an offer is only a claim until the chunks arrive.
        self.active.insert(
            key,
            Incoming {
                offer,
                bytes: Box::<PlainChunks>::default(),
                held: 0,
                next: 0,
            },
        );
        BeginOutcome::Ok
    }

    /// Start a transfer from bytes already held (a checkpoint of an earlier attempt). `bytes` must be a whole number of
    /// chunks and shorter than the file, or the transfer starts from nothing instead.
    pub fn begin_from(&mut self, offer: FileOffer, bytes: Vec<u8>) -> BeginOutcome {
        self.begin_with(offer, bytes, Box::<PlainChunks>::default())
    }

    /// Like `begin_from`, keeping the bytes in `store`.
    pub fn begin_with(
        &mut self,
        offer: FileOffer,
        bytes: Vec<u8>,
        store: Box<dyn ChunkStore>,
    ) -> BeginOutcome {
        let usable =
            !bytes.is_empty() && bytes.len() < offer.size && bytes.len() % FILE_CHUNK_BYTES == 0;
        let key = offer.message_id.to_string();
        let outcome = self.begin(offer);
        if outcome == BeginOutcome::Ok {
            if let Some(t) = self.active.get_mut(&key) {
                t.bytes = store;
                if usable {
                    for part in bytes.chunks(FILE_CHUNK_BYTES) {
                        t.bytes.push(part);
                    }
                    t.held = bytes.len();
                    t.next = bytes.len() / FILE_CHUNK_BYTES;
                }
            }
        }
        outcome
    }

    /// Whether this is the very file already being received (same id, size and hash): a sender that offers it again after
    /// its retry timer ran out is not starting over.
    pub fn holds(&self, offer: &FileOffer) -> bool {
        self.active
            .get(offer.message_id.as_str())
            .is_some_and(|t| t.offer.sha256 == offer.sha256 && t.offer.size == offer.size)
    }

    /// The transfer's offer and chunks `from..to` of what has been received so far (what a checkpoint keeps).
    pub fn partial(&self, id: &MessageId, from: usize, to: usize) -> Option<(&FileOffer, Vec<u8>)> {
        let t = self.active.get(id.as_str())?;
        (t.next > 0).then(|| (&t.offer, t.bytes.range(from, to.min(t.next))))
    }

    /// Ids of the transfers in progress.
    pub fn active_ids(&self) -> Vec<MessageId> {
        self.active
            .values()
            .map(|t| t.offer.message_id.clone())
            .collect()
    }

    pub fn chunk(&mut self, frame: &FileChunk) -> AssembleResult {
        let key = frame.message_id.to_string();
        let Some(t) = self.active.get_mut(&key) else {
            return AssembleResult::Failed(DeclineReason::BadFile);
        };
        // A chunk we already hold (a sender that does not know where we resumed sends from the start): skip it.
        if frame.index < t.next {
            return AssembleResult::More;
        }
        let Some(data) = codec::decode(&frame.data) else {
            self.active.remove(&key);
            return AssembleResult::Failed(DeclineReason::BadFile);
        };
        let size = t.offer.size;
        let expected_last = size.div_ceil(FILE_CHUNK_BYTES) - 1;
        let expected_len = if frame.index == expected_last {
            size - expected_last * FILE_CHUNK_BYTES
        } else {
            FILE_CHUNK_BYTES
        };
        if frame.index != t.next || data.len() != expected_len || t.held + data.len() > size {
            self.active.remove(&key);
            return AssembleResult::Failed(DeclineReason::BadFile);
        }
        t.bytes.push(&data);
        t.held += data.len();
        t.next += 1;
        if t.held < size {
            return AssembleResult::More;
        }
        let Some(mut done) = self.active.remove(&key) else {
            return AssembleResult::Failed(DeclineReason::BadFile);
        };
        let bytes = done.bytes.take();
        if sha256_b64(&bytes) != done.offer.sha256 {
            return AssembleResult::Failed(DeclineReason::BadFile);
        }
        AssembleResult::Done {
            offer: Box::new(done.offer),
            bytes,
        }
    }

    /// The chunk this transfer needs next, or `None` if there is no such transfer.
    pub fn next_index(&self, id: &MessageId) -> Option<usize> {
        self.active.get(id.as_str()).map(|t| t.next)
    }

    /// 0.0 to 1.0, or `None` if there is no such transfer.
    pub fn progress(&self, id: &MessageId) -> Option<f64> {
        let t = self.active.get(id.as_str())?;
        Some(t.held as f64 / t.offer.size as f64)
    }

    pub fn abort(&mut self, id: &MessageId) {
        self.active.remove(id.as_str());
    }

    pub fn clear(&mut self) {
        self.active.clear();
    }
}
