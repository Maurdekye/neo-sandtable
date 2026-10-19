//! Bounded complete-event SSE guard. Safe events release in arrival order;
//! unresolved semantic prefixes hold associated events; safe pings may bypass.
use axum::{body::Bytes, http::HeaderMap};
use std::collections::VecDeque;

struct Pattern {
    value: Vec<u8>,
    fallback: Vec<usize>,
    matched: usize,
}
struct Matcher {
    patterns: Vec<Pattern>,
    origins: VecDeque<usize>,
    max_len: usize,
}
impl Matcher {
    fn new(values: &[Vec<u8>]) -> Self {
        let patterns = values
            .iter()
            .map(|value| {
                let mut fallback = vec![0; value.len()];
                for i in 1..value.len() {
                    let mut n = fallback[i - 1];
                    while n > 0 && value[i] != value[n] {
                        n = fallback[n - 1];
                    }
                    if value[i] == value[n] {
                        n += 1;
                    }
                    fallback[i] = n;
                }
                Pattern {
                    value: value.clone(),
                    fallback,
                    matched: 0,
                }
            })
            .collect();
        Self {
            patterns,
            origins: VecDeque::new(),
            max_len: values.iter().map(Vec::len).max().unwrap_or(0),
        }
    }
    fn feed(&mut self, byte: u8, origin: usize) -> Result<(), ()> {
        if self.max_len == 0 {
            return Ok(());
        }
        self.origins.push_back(origin);
        if self.origins.len() > self.max_len {
            self.origins.pop_front();
        }
        for p in &mut self.patterns {
            while p.matched > 0 && byte != p.value[p.matched] {
                p.matched = p.fallback[p.matched - 1];
            }
            if byte == p.value[p.matched] {
                p.matched += 1;
            }
            if p.matched == p.value.len() {
                return Err(());
            }
        }
        Ok(())
    }
    fn hold(&self) -> Option<usize> {
        self.patterns
            .iter()
            .filter(|p| p.matched > 0)
            .map(|p| self.origins[self.origins.len() - p.matched])
            .min()
    }
    fn clear(&mut self) {
        self.origins.clear();
        for p in &mut self.patterns {
            p.matched = 0;
        }
    }
}

#[derive(Default)]
enum Mode {
    #[default]
    Outside,
    Plain,
    Slash {
        origin: usize,
    },
    Hex {
        origin: usize,
        code: u16,
        digits: u8,
        high: Option<u16>,
    },
    LowSlash {
        origin: usize,
        high: u16,
    },
    LowU {
        origin: usize,
        high: u16,
    },
}
struct Decoded {
    bytes: [u8; 4],
    len: usize,
    origin: usize,
}
#[derive(Default)]
struct JsonScan {
    mode: Mode,
}
impl JsonScan {
    fn inside(&self) -> bool {
        !matches!(self.mode, Mode::Outside)
    }
    fn hold(&self) -> Option<usize> {
        match self.mode {
            Mode::Slash { origin }
            | Mode::Hex { origin, .. }
            | Mode::LowSlash { origin, .. }
            | Mode::LowU { origin, .. } => Some(origin),
            _ => None,
        }
    }
    fn decoded(&mut self, bytes: &[u8], origin: usize) -> Decoded {
        let mut result = Decoded {
            bytes: [0; 4],
            len: bytes.len(),
            origin,
        };
        result.bytes[..bytes.len()].copy_from_slice(bytes);
        result
    }
    fn scalar(&mut self, code: u32, origin: usize) -> Result<Decoded, ()> {
        let ch = char::from_u32(code).ok_or(())?;
        let mut bytes = [0; 4];
        Ok(self.decoded(ch.encode_utf8(&mut bytes).as_bytes(), origin))
    }
    fn feed(&mut self, byte: u8, origin: usize) -> Result<Option<Decoded>, ()> {
        let mode = std::mem::take(&mut self.mode);
        match mode {
            Mode::Outside => {
                if byte == b'"' {
                    self.mode = Mode::Plain;
                }
                Ok(None)
            }
            Mode::Plain => match byte {
                b'"' => Ok(None),
                b'\\' => {
                    self.mode = Mode::Slash { origin };
                    Ok(None)
                }
                0..=31 => Err(()),
                _ => {
                    self.mode = Mode::Plain;
                    Ok(Some(self.decoded(&[byte], origin)))
                }
            },
            Mode::Slash { origin } => {
                if byte == b'u' {
                    self.mode = Mode::Hex {
                        origin,
                        code: 0,
                        digits: 0,
                        high: None,
                    };
                    return Ok(None);
                }
                let decoded = match byte {
                    b'"' => b'"',
                    b'\\' => b'\\',
                    b'/' => b'/',
                    b'b' => 8,
                    b'f' => 12,
                    b'n' => b'\n',
                    b'r' => b'\r',
                    b't' => b'\t',
                    _ => return Err(()),
                };
                self.mode = Mode::Plain;
                Ok(Some(self.decoded(&[decoded], origin)))
            }
            Mode::Hex {
                origin,
                code,
                digits,
                high,
            } => {
                let digit = (byte as char).to_digit(16).ok_or(())? as u16;
                let code = (code << 4) | digit;
                if digits < 3 {
                    self.mode = Mode::Hex {
                        origin,
                        code,
                        digits: digits + 1,
                        high,
                    };
                    return Ok(None);
                }
                if let Some(high) = high {
                    if !(0xdc00..=0xdfff).contains(&code) {
                        return Err(());
                    }
                    self.mode = Mode::Plain;
                    return Ok(Some(self.scalar(
                        0x10000 + (((high as u32 - 0xd800) << 10) | (code as u32 - 0xdc00)),
                        origin,
                    )?));
                }
                if (0xd800..=0xdbff).contains(&code) {
                    self.mode = Mode::LowSlash { origin, high: code };
                    return Ok(None);
                }
                self.mode = Mode::Plain;
                Ok(Some(self.scalar(code as u32, origin)?))
            }
            Mode::LowSlash { origin, high } => {
                if byte != b'\\' {
                    return Err(());
                }
                self.mode = Mode::LowU { origin, high };
                Ok(None)
            }
            Mode::LowU { origin, high } => {
                if byte != b'u' {
                    return Err(());
                }
                self.mode = Mode::Hex {
                    origin,
                    code: 0,
                    digits: 0,
                    high: Some(high),
                };
                Ok(None)
            }
        }
    }
}

// Complete frames are parsed before release. Limits bound malicious framing
// and indefinitely unresolved semantic prefixes.
const BLOCK_LIMIT: usize = 128;
const INDEX_LIMIT: u64 = 1023;
const QUEUE_LIMIT: usize = 1024;
const PROTECTED_LIMIT: usize = 8 * 1024;
const PROTECTED_COUNT_LIMIT: usize = 64;
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Text,
    Thinking,
    Tool,
    Redacted,
}
struct Tool {
    raw: Matcher,
    decoded: Matcher,
    scan: JsonScan,
}
impl Tool {
    fn new(values: &[Vec<u8>]) -> Self {
        Self {
            raw: Matcher::new(values),
            decoded: Matcher::new(values),
            scan: JsonScan::default(),
        }
    }
    fn feed(&mut self, fragment: &str, event: usize) -> Result<(), ()> {
        for b in fragment.bytes() {
            self.raw.feed(b, event)?;
            if let Some(decoded) = self.scan.feed(b, event)? {
                for byte in &decoded.bytes[..decoded.len] {
                    self.decoded.feed(*byte, decoded.origin)?;
                }
            }
            if !self.scan.inside() {
                self.decoded.clear();
            }
        }
        Ok(())
    }
    fn hold(&self) -> Option<usize> {
        [self.raw.hold(), self.decoded.hold(), self.scan.hold()]
            .into_iter()
            .flatten()
            .min()
    }
}
struct Block {
    kind: Kind,
    open: bool,
    tool: Option<Tool>,
    signature: Matcher,
}
/// No Debug/Serialize: protected values and event queues remain request-local.
pub(super) struct ProtectedValues {
    values: Vec<Vec<u8>>,
    invalid: bool,
    raw: Matcher,
    text: Matcher,
    thinking: Matcher,
    blocks: std::collections::BTreeMap<u64, Block>,
    event: Vec<u8>,
    line: Vec<u8>,
    data: Vec<u8>,
    previous_cr: bool,
    initial: bool,
    serial: usize,
    pending: VecDeque<(usize, Vec<u8>)>,
    held_bytes: usize,
    // Emptiness is an EOF safety witness, never diagnostics.
    pub(super) tail: Vec<u8>,
}
impl ProtectedValues {
    pub(super) fn new(headers: &HeaderMap) -> Self {
        let mut values = Vec::new();
        let mut invalid = false;
        for name in ["authorization", "anthropic-beta"] {
            if let Some(value) = headers.get(name) {
                if value.as_bytes().len() > PROTECTED_LIMIT {
                    invalid = true;
                    break;
                }
                values.push(value.as_bytes().to_vec());
                if name == "authorization" {
                    if let Some(token) = value.as_bytes().strip_prefix(b"Bearer ") {
                        values.push(token.to_vec());
                    }
                } else if let Ok(value) = value.to_str() {
                    for part in value.split(',').map(str::trim).filter(|s| !s.is_empty()) {
                        if values.len() >= PROTECTED_COUNT_LIMIT {
                            invalid = true;
                            break;
                        }
                        values.push(part.as_bytes().to_vec());
                    }
                }
            }
        }
        values.retain(|v| !v.is_empty());
        invalid |= values.len() > PROTECTED_COUNT_LIMIT
            || values.iter().map(Vec::len).sum::<usize>() > PROTECTED_LIMIT;
        if invalid {
            values.clear();
        }
        Self {
            raw: Matcher::new(&values),
            text: Matcher::new(&values),
            thinking: Matcher::new(&values),
            values,
            invalid,
            blocks: Default::default(),
            event: Vec::new(),
            line: Vec::new(),
            data: Vec::new(),
            previous_cr: false,
            initial: true,
            serial: 0,
            pending: VecDeque::new(),
            held_bytes: 0,
            tail: Vec::new(),
        }
    }
    pub(super) fn contains(&self, bytes: &[u8]) -> bool {
        self.invalid
            || self
                .values
                .iter()
                .any(|v| bytes.windows(v.len()).any(|w| w == v))
    }
    fn check_value(&self, value: &serde_json::Value) -> Result<(), ()> {
        use serde_json::Value;
        match value {
            Value::String(s) => {
                if self.contains(s.as_bytes()) {
                    return Err(());
                }
            }
            Value::Array(a) => {
                for v in a {
                    self.check_value(v)?;
                }
            }
            Value::Object(o) => {
                for (k, v) in o {
                    if self.contains(k.as_bytes()) {
                        return Err(());
                    }
                    self.check_value(v)?;
                }
            }
            _ => {}
        }
        Ok(())
    }
    fn feed(matcher: &mut Matcher, text: &str, event: usize) -> Result<(), ()> {
        for b in text.bytes() {
            matcher.feed(b, event)?;
        }
        Ok(())
    }
    fn index(value: &serde_json::Value) -> Result<u64, ()> {
        value["index"]
            .as_u64()
            .filter(|n| *n <= INDEX_LIMIT)
            .ok_or(())
    }
    fn protocol(&mut self, value: &serde_json::Value) -> Result<(), ()> {
        let event = self.serial;
        match value["type"].as_str().ok_or(())? {
            "content_block_start" => {
                let index = Self::index(value)?;
                if self.blocks.contains_key(&index) || self.blocks.len() >= BLOCK_LIMIT {
                    return Err(());
                }
                let content = &value["content_block"];
                let object = content.as_object().ok_or(())?;
                let allowed: &[&str] = match content["type"].as_str() {
                    Some("text") => &["text"],
                    Some("thinking") => &["thinking", "signature"],
                    Some("tool_use") => &["input"],
                    Some("redacted_thinking") => &["data"],
                    _ => return Err(()),
                };
                if [
                    "text",
                    "thinking",
                    "signature",
                    "input",
                    "partial_json",
                    "data",
                ]
                .iter()
                .any(|field| object.contains_key(*field) && !allowed.contains(field))
                {
                    return Err(());
                }
                let kind = match content["type"].as_str() {
                    Some("text") => {
                        Self::feed(&mut self.text, content["text"].as_str().ok_or(())?, event)?;
                        Kind::Text
                    }
                    Some("thinking") => {
                        Self::feed(
                            &mut self.thinking,
                            content["thinking"].as_str().ok_or(())?,
                            event,
                        )?;
                        Kind::Thinking
                    }
                    Some("tool_use") => {
                        if !content["input"].is_object() {
                            return Err(());
                        }
                        Kind::Tool
                    }
                    Some("redacted_thinking") => {
                        content["data"].as_str().ok_or(())?;
                        Kind::Redacted
                    }
                    _ => return Err(()),
                };
                let mut signature = Matcher::new(&self.values);
                if let Some(initial) = content.get("signature") {
                    Self::feed(&mut signature, initial.as_str().ok_or(())?, event)?;
                }
                self.blocks.insert(
                    index,
                    Block {
                        kind,
                        open: true,
                        tool: (kind == Kind::Tool).then(|| Tool::new(&self.values)),
                        signature,
                    },
                );
            }
            "content_block_delta" => {
                let index = Self::index(value)?;
                let block = self.blocks.get_mut(&index).filter(|b| b.open).ok_or(())?;
                let delta = &value["delta"];
                let object = delta.as_object().ok_or(())?;
                if object.len() != 2 || !object.contains_key("type") {
                    return Err(());
                }
                match (block.kind, delta["type"].as_str()) {
                    (Kind::Text, Some("text_delta")) => {
                        Self::feed(&mut self.text, delta["text"].as_str().ok_or(())?, event)?
                    }
                    (Kind::Thinking, Some("thinking_delta")) => Self::feed(
                        &mut self.thinking,
                        delta["thinking"].as_str().ok_or(())?,
                        event,
                    )?,
                    (Kind::Thinking, Some("signature_delta")) => Self::feed(
                        &mut block.signature,
                        delta["signature"].as_str().ok_or(())?,
                        event,
                    )?,
                    (Kind::Tool, Some("input_json_delta")) => block
                        .tool
                        .as_mut()
                        .ok_or(())?
                        .feed(delta["partial_json"].as_str().ok_or(())?, event)?,
                    _ => return Err(()),
                }
            }
            "content_block_stop" => {
                let index = Self::index(value)?;
                let block = self.blocks.get_mut(&index).filter(|b| b.open).ok_or(())?;
                block.open = false;
                if block
                    .tool
                    .as_ref()
                    .is_some_and(|t| t.hold().is_some() || t.scan.inside())
                    || block.signature.hold().is_some()
                {
                    return Err(());
                }
                block.tool = None;
            }
            "message_delta" | "message_stop" => {
                if self.blocks.values().any(|b| b.open) {
                    return Err(());
                }
            }
            "message_start" | "ping" | "error" => {}
            _ => return Err(()),
        }
        Ok(())
    }
    fn hold(&self) -> Option<usize> {
        [self.text.hold(), self.thinking.hold()]
            .into_iter()
            .flatten()
            .chain(
                self.blocks
                    .values()
                    .filter_map(|b| b.tool.as_ref().and_then(Tool::hold)),
            )
            .chain(self.blocks.values().filter_map(|b| b.signature.hold()))
            .min()
    }
    fn complete(
        &mut self,
        output: &mut Vec<u8>,
        arrival: &mut impl FnMut(&[u8]) -> Result<(), ()>,
    ) -> Result<(), ()> {
        if self.event.len() > super::EVENT_LIMIT {
            return Err(());
        }
        if self.contains(&self.event) {
            return Err(());
        }
        let value = if self.data.is_empty() {
            None
        } else {
            Some(super::strict_json::parse(&self.data)?)
        };
        if let Some(value) = &value {
            self.check_value(value)?;
            self.protocol(value)?;
        }
        arrival(&self.event)?;
        if value.as_ref().is_some_and(|v| v["type"] == "message_stop") && self.hold().is_some() {
            return Err(());
        }
        let ping = value
            .as_ref()
            .is_some_and(|v| v["type"] == "ping" && v.as_object().is_some_and(|o| o.len() == 1))
            || (value.is_none() && self.event.iter().all(|b| b.is_ascii_whitespace()));
        let wire = std::mem::take(&mut self.event);
        self.data.clear();
        if ping {
            output.extend(wire);
        } else {
            if self.pending.len() >= QUEUE_LIMIT {
                return Err(());
            }
            self.held_bytes = self.held_bytes.checked_add(wire.len()).ok_or(())?;
            self.pending.push_back((self.serial, wire));
        }
        self.serial = self.serial.checked_add(1).ok_or(())?;
        let hold = self.hold().unwrap_or(self.serial);
        while self.pending.front().is_some_and(|(id, _)| *id < hold) {
            let (_, wire) = self.pending.pop_front().ok_or(())?;
            self.held_bytes = self.held_bytes.checked_sub(wire.len()).ok_or(())?;
            output.extend(wire);
        }
        Ok(())
    }
    fn newline(
        &mut self,
        output: &mut Vec<u8>,
        arrival: &mut impl FnMut(&[u8]) -> Result<(), ()>,
    ) -> Result<(), ()> {
        let mut line = std::mem::take(&mut self.line);
        if self.initial {
            self.initial = false;
            if line.starts_with(b"\xef\xbb\xbf") {
                line.drain(..3);
            }
        }
        if line.is_empty() {
            return self.complete(output, arrival);
        }
        if let Some(data) = line.strip_prefix(b"data:") {
            let data = data.strip_prefix(b" ").unwrap_or(data);
            if !self.data.is_empty() {
                self.data.push(b'\n');
            }
            self.data.extend_from_slice(data);
        }
        Ok(())
    }
    fn witness(&mut self) -> Result<(), ()> {
        let n = self.event.len().checked_add(self.held_bytes).ok_or(())?;
        if n > super::EVENT_LIMIT {
            return Err(());
        }
        self.tail.clear();
        if n > 0 {
            self.tail.push(0);
        }
        Ok(())
    }
    #[cfg(test)]
    pub(super) fn push(&mut self, bytes: &[u8]) -> Result<Bytes, ()> {
        self.push_with(bytes, |_| Ok(()))
    }
    pub(super) fn push_with(
        &mut self,
        bytes: &[u8],
        mut arrival: impl FnMut(&[u8]) -> Result<(), ()>,
    ) -> Result<Bytes, ()> {
        if self.invalid {
            return Err(());
        }
        let mut output = Vec::new();
        for &byte in bytes {
            self.raw.feed(byte, self.serial)?;
            if self.previous_cr {
                self.previous_cr = false;
                if byte == b'\n' {
                    self.event.push(byte);
                    self.witness()?;
                    self.newline(&mut output, &mut arrival)?;
                    continue;
                }
                self.newline(&mut output, &mut arrival)?;
            }
            self.event.push(byte);
            self.witness()?;
            match byte {
                b'\r' => self.previous_cr = true,
                b'\n' => self.newline(&mut output, &mut arrival)?,
                _ => self.line.push(byte),
            }
            self.witness()?;
        }
        self.witness()?;
        Ok(Bytes::from(output))
    }
    #[cfg(test)]
    pub(super) fn finish(&mut self) -> Result<Bytes, ()> {
        self.finish_with(|_| Ok(()))
    }
    pub(super) fn finish_with(
        &mut self,
        mut arrival: impl FnMut(&[u8]) -> Result<(), ()>,
    ) -> Result<Bytes, ()> {
        let mut output = Vec::new();
        if self.previous_cr {
            self.previous_cr = false;
            self.newline(&mut output, &mut arrival)?;
        }
        self.witness()?;
        if !self.tail.is_empty() || self.hold().is_some() || self.blocks.values().any(|b| b.open) {
            return Err(());
        }
        Ok(Bytes::from(output))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    const AUTH: &str = "AUTH_CANARY_REFLECTION_92";
    const BETA: &str = "BETA_CAPABILITY_CANARY_73";
    fn guard() -> ProtectedValues {
        let mut headers = HeaderMap::new();
        headers.insert("authorization", format!("Bearer {AUTH}").parse().unwrap());
        headers.insert("anthropic-beta", BETA.parse().unwrap());
        ProtectedValues::new(&headers)
    }
    fn escaped(value: &str) -> String {
        value
            .chars()
            .map(|c| format!("\\u{:04x}", c as u32))
            .collect()
    }
    fn frame(key: &str, value: &str) -> Vec<u8> {
        format!("data: {{\"{key}\":\"{value}\"}}\n\n").into_bytes()
    }
    fn rejected(chunks: Vec<Vec<u8>>) {
        let mut guard = guard();
        let mut released = Vec::new();
        let mut refused = false;
        for chunk in chunks {
            match guard.push(&chunk) {
                Ok(bytes) => released.extend_from_slice(&bytes),
                Err(()) => {
                    refused = true;
                    break;
                }
            }
        }
        if !refused {
            refused = guard.finish().is_err();
        }
        assert!(refused);
        // A normal consumer only gets completed data frames, never the held
        // protected prefix or a completed string containing its decoded value.
        for event in released.split(|b| *b == b'\n') {
            if let Some(data) = event.strip_prefix(b"data: ")
                && let Ok(value) = serde_json::from_slice::<serde_json::Value>(data)
            {
                let decoded = value.to_string();
                assert!(!decoded.contains(AUTH) && !decoded.contains(BETA));
            }
        }
    }
    #[test]
    fn decoded_auth_and_capability_refuse_every_split_and_bytewise_escape() {
        for secret in [AUTH, BETA] {
            for body in [
                escaped(secret),
                format!("{}{}", &secret[..4], escaped(&secret[4..])),
            ] {
                let wire = frame("text", &body);
                for cut in 0..=wire.len() {
                    rejected(vec![wire[..cut].to_vec(), wire[cut..].to_vec()]);
                }
                rejected(wire.iter().map(|b| vec![*b]).collect());
            }
        }
    }
    #[test]
    fn cr_lf_crlf_bom_and_escaped_key_match_normal_sse_decoding() {
        for ending in ["\n", "\r", "\r\n"] {
            let wire = format!(
                "\u{feff}data: {{\"te\\u0078t\":\"{}\"}}{ending}{ending}",
                escaped(AUTH)
            );
            rejected(wire.as_bytes().iter().map(|b| vec![*b]).collect());
        }
    }
    #[test]
    fn text_and_nested_partial_json_secrets_across_frames_stay_withheld() {
        rejected(vec![
            frame("text", &escaped(&AUTH[..7])),
            frame("text", &escaped(&AUTH[7..])),
        ]);
        let nested = format!("{{\"value\":\"{}\"}}", escaped(BETA));
        let chunks = nested
            .as_bytes()
            .chunks(3)
            .map(|chunk| {
                let value = std::str::from_utf8(chunk).unwrap();
                format!("data: {}\n\n", serde_json::json!({"partial_json":value})).into_bytes()
            })
            .collect();
        rejected(chunks);
    }

    #[test]
    fn complete_event_and_ping_release_before_stream_completion() {
        let mut guard = guard();
        let ping = b"event: ping\ndata: {\"type\":\"ping\"}\n\n";
        assert_eq!(guard.push(ping).unwrap().as_ref(), ping);
        let partial = b"data: {\"type\":\"ping\",\"note\":\"clear";
        assert!(guard.push(partial).unwrap().is_empty());
        assert!(!guard.tail.is_empty());
        let rest = b"!\"}\n\n";
        assert_eq!(
            guard.push(rest).unwrap().as_ref(),
            [partial.as_slice(), rest].concat()
        );
        assert!(guard.tail.is_empty());
        assert!(guard.finish().unwrap().is_empty());
    }
    #[test]
    fn benign_surrogate_escape_is_held_until_event_is_complete() {
        let mut guard = guard();
        let partial = b"data: {\"type\":\"ping\",\"note\":\"\\ud83d";
        assert!(guard.push(partial).unwrap().is_empty());
        let rest = b"\\ude80\"}\n\n";
        assert_eq!(
            guard.push(rest).unwrap().as_ref(),
            [partial.as_slice(), rest].concat()
        );
        assert!(guard.finish().unwrap().is_empty());
    }
    #[test]
    fn invalid_escape_and_excessive_pending_prefix_fail_closed() {
        assert!(guard().push(b"data: {\"text\":\"\\uq123\"}\n\n").is_err());
        let mut guard = guard();
        let first = format!(
            "data: {}\n\n",
            serde_json::json!({
                "type":"content_block_start","index":0,
                "content_block":{"type":"text","text":&AUTH[..1]}
            })
        )
        .into_bytes();
        guard.push(&first).unwrap();
        assert!(!guard.tail.is_empty());
        assert!(
            guard
                .push(&vec![b' '; super::super::EVENT_LIMIT + 1])
                .is_err()
        );
    }
}
