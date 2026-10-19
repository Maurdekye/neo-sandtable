//! Private installed Claude Code 2.1.292 wire contract (lead m932, Platform m501).
//! Source receipts: claude-installed-contract-source-20261013-0001.json in Seats
//! scratch. ACK is input replay, NOT a fresh query, terminal or healthy receipt.
//! Later frames may omit the documented first stamp; terminal must restamp.
//! This deliberately refuses unsupported metadata/order rather than guessing.

use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Refusal {
    Ordering,
    Identity,
    Stamp,
    Unsupported,
    Malformed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
enum Phase {
    Unwritten,
    AwaitingAck,
    Active,
    Terminal,
    Failed(Refusal),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Frame {
    Parse,
    ReplayAck,
}

/// Canonical scalar metadata lives in ORIGINAL Operation State, not in an
/// unowned parser mirror. No heap allocation, callback, I/O or body consumption.
pub(crate) struct Correlation {
    expected: [u8; 36],
    phase: Phase,
    init_seen: bool,
    assistant_seen: bool,
    partial_seen: bool,
}

#[derive(serde::Serialize)]
struct BorrowedCorrelation<'a> {
    runtime_associated: &'a bool,
    expected: &'a [u8],
    phase: &'a Phase,
    init_seen: &'a bool,
    assistant_seen: &'a bool,
    partial_seen: &'a bool,
}

pub(crate) const fn reserved_bytes() -> usize {
    std::mem::size_of::<Option<Correlation>>()
        // Canonical checkout and bounded borrowed serialization projection can
        // overlap. Neither clones metadata nor retains a shadow collection.
        + std::mem::size_of::<crate::operation::CorrelationArchiveLease<'_>>()
        + std::mem::size_of::<BorrowedCorrelation<'_>>()
        + std::mem::size_of::<bool>()
        // Parser's original association, installed once before prompt effects.
        + std::mem::size_of::<Option<crate::operation::Operation>>()
        // Creation UUID/formatter headers plus overlapping prompt UUID copy
        // and borrowed views in the caller and its input frame. No heap body.
        + std::mem::size_of::<uuid::Uuid>()
        + std::mem::size_of::<uuid::fmt::Hyphenated>()
        + 36
        + 2 * std::mem::size_of::<&str>()
}

impl Correlation {
    /// Private boundary prerequisite only. Input replay/first reply cannot stand
    /// in for the independently stamped terminal. This never resets the flight.
    pub(crate) fn check_terminal(&self) -> Result<(), Refusal> {
        match self.phase {
            Phase::Terminal => Ok(()),
            Phase::Failed(first) => Err(first),
            _ => Err(Refusal::Ordering),
        }
    }

    pub(crate) fn visit_archive(
        &self,
        original: &crate::operation::Operation,
        runtime_associated: bool,
        sink: &mut impl super::archive_stdout::RecordSink,
    ) -> Result<(), crate::failed_evidence::ArchiveError> {
        use crate::failed_evidence::{EvidenceCategory, EvidenceRecord};
        // Logical original metadata has no inferred physical read origin.
        // Every canonical field is included, including first refusal and phase.
        sink.json(
            EvidenceRecord {
                operation: original,
                category: EvidenceCategory::Diagnostic,
                part: "claude_input_correlation",
                index: 0,
                physical_child: None,
                origin: None,
            },
            &BorrowedCorrelation {
                runtime_associated: &runtime_associated,
                expected: &self.expected,
                phase: &self.phase,
                init_seen: &self.init_seen,
                assistant_seen: &self.assistant_seen,
                partial_seen: &self.partial_seen,
            },
        )
    }

    /// Only called after ORIGINAL creation admission, never after overflow.
    pub(crate) fn new() -> Self {
        let mut expected = [0; 36];
        uuid::Uuid::new_v4()
            .hyphenated()
            .encode_lower(&mut expected);
        Self {
            expected,
            phase: Phase::Unwritten,
            init_seen: false,
            assistant_seen: false,
            partial_seen: false,
        }
    }

    /// Irreversible preparation BEFORE the cancelable prompt write. A refused
    /// or canceled write cannot redispatch/replace this original input UUID.
    pub(crate) fn prepare(&mut self) -> Result<[u8; 36], Refusal> {
        if self.phase != Phase::Unwritten {
            return Err(self.fail(Refusal::Ordering));
        }
        self.phase = Phase::AwaitingAck;
        Ok(self.expected)
    }

    pub(crate) fn fail(&mut self, reason: Refusal) -> Refusal {
        if let Phase::Failed(first) = self.phase {
            return first;
        }
        self.phase = Phase::Failed(reason);
        reason
    }

    pub(crate) fn validate(&mut self, v: &Value) -> Result<Frame, Refusal> {
        if let Phase::Failed(first) = self.phase {
            return Err(first);
        }
        let result = self.check(v);
        if let Err(reason) = result {
            self.fail(reason);
        }
        result
    }

    fn stamp(&self, v: &Value, required: bool) -> Result<(), Refusal> {
        let singular = v.get("user_message_uuid");
        let plural = v.get("user_message_uuids");
        if singular.is_none() && plural.is_none() && !required {
            return Ok(());
        }
        if singular.and_then(Value::as_str).map(str::as_bytes) != Some(&self.expected[..]) {
            return Err(Refusal::Stamp);
        }
        if let Some(plural) = plural {
            let ids = plural.as_array().ok_or(Refusal::Stamp)?;
            if ids.len() != 1 || ids[0].as_str().map(str::as_bytes) != Some(&self.expected[..]) {
                return Err(Refusal::Stamp);
            }
        }
        Ok(())
    }

    fn check(&mut self, v: &Value) -> Result<Frame, Refusal> {
        let object = v.as_object().ok_or(Refusal::Malformed)?;
        if matches!(self.phase, Phase::Unwritten | Phase::Terminal) {
            return Err(Refusal::Ordering);
        }
        let kind = v
            .get("type")
            .and_then(Value::as_str)
            .ok_or(Refusal::Malformed)?;
        // Every unlisted wrapper field is unsupported. In particular rescue,
        // historical, subagent, synthetic-origin and folded-input metadata do
        // not silently acquire a local Operation merely by arriving in stdout.
        let allowed: &[&str] = match kind {
            "system" => &[
                "type",
                "subtype",
                "uuid",
                "session_id",
                "cwd",
                "tools",
                "mcp_servers",
                "model",
                "permissionMode",
                "slash_commands",
                "apiKeySource",
                "claude_code_version",
                "output_style",
                "agents",
                "skills",
                "plugins",
                "fast_mode_state",
                "betas",
            ],
            "user" => &[
                "type",
                "message",
                "uuid",
                "session_id",
                "parent_tool_use_id",
                "isReplay",
                "timestamp",
                "isSynthetic",
                "user_message_uuid",
                "user_message_uuids",
            ],
            "assistant" => &[
                "type",
                "message",
                "uuid",
                "session_id",
                "parent_tool_use_id",
                "request_id",
                "user_message_uuid",
                "user_message_uuids",
            ],
            "stream_event" => &[
                "type",
                "event",
                "uuid",
                "session_id",
                "parent_tool_use_id",
                "user_message_uuid",
                "user_message_uuids",
            ],
            "result" => &[
                "type",
                "subtype",
                "is_error",
                "duration_ms",
                "duration_api_ms",
                "num_turns",
                "stop_reason",
                "result",
                "usage",
                "modelUsage",
                "total_cost_usd",
                "permission_denials",
                "errors",
                "uuid",
                "session_id",
                "user_message_uuid",
                "user_message_uuids",
            ],
            "rate_limit_event" => &[
                "type",
                "rate_limit_info",
                "uuid",
                "session_id",
                "user_message_uuid",
                "user_message_uuids",
            ],
            _ => return Err(Refusal::Unsupported),
        };
        if object.keys().any(|key| !allowed.contains(&key.as_str())) {
            return Err(Refusal::Unsupported);
        }
        if kind == "system" {
            // Only explicitly identified startup init is non-turn metadata.
            // The existing isolation/model checks still run on this frame.
            if self.phase != Phase::AwaitingAck
                || self.init_seen
                || v["subtype"] != "init"
                || v["claude_code_version"] != "2.1.292"
                || !v["session_id"].as_str().is_some_and(|s| !s.is_empty())
                || !v["tools"].is_array()
            {
                return Err(Refusal::Unsupported);
            }
            self.init_seen = true;
            return Ok(Frame::Parse);
        }
        if kind == "user" && v.get("isReplay").is_some() {
            if self.phase != Phase::AwaitingAck {
                return Err(Refusal::Ordering);
            }
            if v["isReplay"] != true
                || v["uuid"].as_str().map(str::as_bytes) != Some(&self.expected[..])
                || v.get("parent_tool_use_id") != Some(&Value::Null)
                || v["message"]["role"] != "user"
                || !replayed_text(&v["message"]["content"])
                || !v["session_id"].as_str().is_some_and(|s| !s.is_empty())
                || v.get("isSynthetic")
                    .is_some_and(|value| value.as_bool() != Some(false))
            {
                return Err(Refusal::Identity);
            }
            self.stamp(v, false)?;
            self.phase = Phase::Active;
            return Ok(Frame::ReplayAck);
        }
        if self.phase != Phase::Active {
            return Err(Refusal::Ordering);
        }
        match kind {
            "assistant" | "stream_event" | "user" => {
                if v.get("parent_tool_use_id") != Some(&Value::Null) {
                    return Err(Refusal::Unsupported);
                }
                match kind {
                    "assistant" => {
                        if v["message"]["role"] != "assistant"
                            || !v["message"]["content"].is_array()
                        {
                            return Err(Refusal::Malformed);
                        }
                        self.stamp(v, !self.assistant_seen)?;
                        self.assistant_seen = true;
                    }
                    "stream_event" => {
                        let event = v["event"]["type"].as_str().ok_or(Refusal::Malformed)?;
                        if ![
                            "ping",
                            "message_start",
                            "message_delta",
                            "message_stop",
                            "content_block_start",
                            "content_block_delta",
                            "content_block_stop",
                        ]
                        .contains(&event)
                        {
                            return Err(Refusal::Unsupported);
                        }
                        self.stamp(
                            v,
                            if event == "ping" {
                                !self.assistant_seen && !self.partial_seen
                            } else {
                                !self.partial_seen
                            },
                        )?;
                        if event != "ping" {
                            self.partial_seen = true;
                        }
                    }
                    "user" => {
                        let blocks = v["message"]["content"]
                            .as_array()
                            .ok_or(Refusal::Unsupported)?;
                        if !self.assistant_seen
                            || v["message"]["role"] != "user"
                            || blocks.is_empty()
                            || v.get("isSynthetic")
                                .is_some_and(|value| value.as_bool() != Some(false))
                            || blocks.iter().any(|block| {
                                block["type"] != "tool_result"
                                    || !block["tool_use_id"]
                                        .as_str()
                                        .is_some_and(|id| !id.is_empty())
                            })
                        {
                            return Err(Refusal::Unsupported);
                        }
                        self.stamp(v, false)?;
                    }
                    _ => unreachable!(),
                }
            }
            "result" => {
                self.stamp(v, true)?;
                self.phase = Phase::Terminal;
            }
            "rate_limit_event" => {
                self.stamp(v, !self.assistant_seen && !self.partial_seen)?;
            }
            _ => return Err(Refusal::Unsupported),
        }
        Ok(Frame::Parse)
    }
}

fn replayed_text(content: &Value) -> bool {
    content.is_string()
        || content.as_array().is_some_and(|blocks| {
            !blocks.is_empty()
                && blocks
                    .iter()
                    .all(|block| block["type"] == "text" && block["text"].is_string())
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn flight() -> (Correlation, String) {
        let mut c = Correlation::new();
        let uuid = String::from_utf8(c.prepare().unwrap().to_vec()).unwrap();
        (c, uuid)
    }
    fn ack(uuid: &str) -> Value {
        json!({"type":"user","isReplay":true,"parent_tool_use_id":null,"session_id":"fixture",
            "uuid":uuid,"isSynthetic":false,"message":{"role":"user","content":"fixture"}})
    }
    fn assistant(uuid: Option<&str>) -> Value {
        let mut v = json!({"type":"assistant","parent_tool_use_id":null,
            "message":{"role":"assistant","content":[]}});
        if let Some(uuid) = uuid {
            v["user_message_uuid"] = json!(uuid);
        }
        v
    }
    fn terminal(uuid: Option<&str>) -> Value {
        let mut v = json!({"type":"result","subtype":"success"});
        if let Some(uuid) = uuid {
            v["user_message_uuid"] = json!(uuid);
        }
        v
    }

    #[test]
    fn ack_then_first_stamp_later_unstamped_and_terminal_restamp() {
        let (mut c, uuid) = flight();
        assert_eq!(c.validate(&ack(&uuid)), Ok(Frame::ReplayAck));
        assert_eq!(c.phase, Phase::Active); // ACK alone never completes.
        assert_eq!(c.validate(&assistant(Some(&uuid))), Ok(Frame::Parse));
        assert_eq!(c.validate(&assistant(None)), Ok(Frame::Parse));
        assert_eq!(c.validate(&terminal(Some(&uuid))), Ok(Frame::Parse));
        assert_eq!(c.validate(&assistant(None)), Err(Refusal::Ordering));
    }

    #[test]
    fn boundary_requires_terminal_and_preserves_first_failure() {
        let mut fresh = Correlation::new();
        assert_eq!(fresh.check_terminal(), Err(Refusal::Ordering));
        let uuid = String::from_utf8(fresh.prepare().unwrap().to_vec()).unwrap();
        assert_eq!(fresh.check_terminal(), Err(Refusal::Ordering));
        fresh.validate(&ack(&uuid)).unwrap();
        assert_eq!(fresh.check_terminal(), Err(Refusal::Ordering));
        fresh.validate(&assistant(Some(&uuid))).unwrap();
        assert_eq!(fresh.check_terminal(), Err(Refusal::Ordering));
        fresh.validate(&terminal(Some(&uuid))).unwrap();
        assert_eq!(fresh.check_terminal(), Ok(()));
        fresh.validate(&assistant(None)).unwrap_err();
        assert_eq!(fresh.check_terminal(), Err(Refusal::Ordering));
        fresh.fail(Refusal::Malformed);
        assert_eq!(fresh.check_terminal(), Err(Refusal::Ordering)); // First cause unchanged.
    }

    #[test]
    fn event_or_result_before_ack_keeps_first_refusal() {
        for is_terminal in [false, true] {
            let (mut c, uuid) = flight();
            let v = if is_terminal {
                terminal(Some(&uuid))
            } else {
                assistant(Some(&uuid))
            };
            assert_eq!(c.validate(&v), Err(Refusal::Ordering));
            assert_eq!(c.validate(&ack(&uuid)), Err(Refusal::Ordering));
            assert_eq!(v["user_message_uuid"], uuid); // Borrowed body untouched.
        }
    }

    #[test]
    fn duplicate_ack_cannot_prove_fresh_query_or_terminal() {
        let (mut c, uuid) = flight();
        c.validate(&ack(&uuid)).unwrap();
        assert_eq!(c.phase, Phase::Active);
        assert_eq!(c.validate(&ack(&uuid)), Err(Refusal::Ordering));
        assert_eq!(c.validate(&terminal(Some(&uuid))), Err(Refusal::Ordering));
        assert_eq!(c.prepare(), Err(Refusal::Ordering));
    }

    #[test]
    fn prepared_but_unwritten_cancellation_and_unstamped_pre_first_metadata_stay_closed() {
        let (mut canceled, _) = flight(); // Simulated cancellation before write/ACK.
        assert_eq!(canceled.prepare(), Err(Refusal::Ordering));
        for kind in ["stream_event", "rate_limit_event"] {
            let (mut c, uuid) = flight();
            c.validate(&ack(&uuid)).unwrap();
            let v = if kind == "stream_event" {
                json!({"type":kind,"parent_tool_use_id":null,"event":{"type":"ping"}})
            } else {
                json!({"type":kind,"rate_limit_info":{}})
            };
            assert_eq!(c.validate(&v), Err(Refusal::Stamp));
        }
    }

    #[test]
    fn singleton_consumed_uuid_and_text_block_replay_accept_but_tool_block_echo_refuses() {
        let (mut c, uuid) = flight();
        let mut replay = ack(&uuid);
        replay["message"]["content"] = json!([{"type":"text","text":"fixture"}]);
        assert_eq!(c.validate(&replay), Ok(Frame::ReplayAck));
        let mut first = assistant(Some(&uuid));
        first["user_message_uuids"] = json!([uuid]);
        assert_eq!(c.validate(&first), Ok(Frame::Parse));
        assert_eq!(c.validate(&terminal(Some(&uuid))), Ok(Frame::Parse));
        let (mut c, uuid) = flight();
        let mut replay = ack(&uuid);
        replay["message"]["content"] = json!([{"type":"tool_result","tool_use_id":"not-input"}]);
        assert_eq!(c.validate(&replay), Err(Refusal::Identity));
    }

    #[test]
    fn stale_uuid_extra_consumed_input_and_missing_first_or_terminal_stamp_refuse() {
        for mode in 0..5 {
            let (mut c, uuid) = flight();
            c.validate(&ack(&uuid)).unwrap();
            let mut v = match mode {
                0 => assistant(Some("stale")),
                1 => assistant(Some(&uuid)),
                2 => assistant(None),
                3 => terminal(None),
                _ => terminal(Some("stale")),
            };
            if mode == 1 {
                v["user_message_uuids"] = json!([uuid, "extra"]);
            }
            assert_eq!(c.validate(&v), Err(Refusal::Stamp));
        }
        let (mut c, _) = flight();
        assert_eq!(c.validate(&ack("stale")), Err(Refusal::Identity));
    }

    #[test]
    fn partial_first_stamp_is_independent_and_subagent_rescue_unsolicited_are_refused() {
        let (mut c, uuid) = flight();
        c.validate(&ack(&uuid)).unwrap();
        c.validate(&assistant(Some(&uuid))).unwrap();
        let v = json!({"type":"stream_event","parent_tool_use_id":null,"event":{"type":"message_start"}});
        assert_eq!(c.validate(&v), Err(Refusal::Stamp));
        let (mut c, uuid) = flight();
        c.validate(&ack(&uuid)).unwrap();
        let mut first = v.clone();
        first["user_message_uuid"] = json!(uuid);
        assert_eq!(c.validate(&first), Ok(Frame::Parse));
        assert_eq!(c.validate(&v), Ok(Frame::Parse)); // Only after independent first partial stamp.
        assert_eq!(c.validate(&assistant(None)), Err(Refusal::Stamp)); // Assistant still needs own first stamp.
        for mode in 0..3 {
            let (mut c, uuid) = flight();
            c.validate(&ack(&uuid)).unwrap();
            let mut v = assistant(Some(&uuid));
            match mode {
                0 => v["parent_tool_use_id"] = json!("subagent"),
                1 => v["resume_reason"] = json!("interrupted_turn"),
                _ => v["unknown_metadata"] = json!(true),
            }
            assert_eq!(c.validate(&v), Err(Refusal::Unsupported));
        }
    }
}
