//! Validated launcher choices; no CLI or database is touched while parsing.
use crate::budget::{RunControl, SpendBudget};
use cna_core::ids::SeatId;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum GameKind {
    Sandbox,
    Cna,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Controller {
    Claude(String),
    Scripted(String),
    Human,
}
impl Controller {
    fn parse(value: &str) -> Result<Self, String> {
        if value == "human" {
            return Ok(Self::Human);
        }
        let (kind, name) = value
            .split_once(':')
            .ok_or("controller must be claude:MODEL, scripted:MODE or human")?;
        match kind {
            "claude"
                if !name.is_empty()
                    && name.as_bytes()[0].is_ascii_alphanumeric()
                    && name
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || "._-".contains(c)) =>
            {
                Ok(Self::Claude(name.into()))
            }
            "scripted" if ["legal_random", "pass_when_possible", "aggressive"].contains(&name) => {
                Ok(Self::Scripted(name.into()))
            }
            _ => Err(format!(
                "unsupported controller {value:?}; Codex/Antigravity are not yet confined"
            )),
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LaunchConfig {
    pub kind: GameKind,
    pub seats: BTreeMap<SeatId, Controller>,
    pub max_turns: usize,
    pub tool_calls: u64,
    pub session: Option<SessionLimits>,
    #[serde(default)]
    pub run: Option<RunControl>,
}
impl Default for LaunchConfig {
    fn default() -> Self {
        Self::resolve(GameKind::Sandbox, &[]).unwrap()
    }
}
impl LaunchConfig {
    pub fn resolve(kind: GameKind, specs: &[String]) -> Result<Self, String> {
        let config = Self::resolve_bindings(kind, specs)?;
        config.validate()?;
        Ok(config)
    }
    fn resolve_bindings(kind: GameKind, specs: &[String]) -> Result<Self, String> {
        let fallback = Controller::Scripted(
            if kind == GameKind::Cna {
                "legal_random"
            } else {
                "aggressive"
            }
            .into(),
        );
        let mut wildcard = None;
        let mut explicit = BTreeMap::new();
        for spec in specs {
            let (seat, controller) = spec
                .split_once('=')
                .ok_or("--seat requires SEAT=CONTROLLER")?;
            let controller = Controller::parse(controller)?;
            if seat == "*" {
                if wildcard.replace(controller).is_some() {
                    return Err("duplicate wildcard binding".into());
                }
            } else {
                let seat: SeatId = seat
                    .parse()
                    .map_err(|e: cna_core::ids::IdError| e.to_string())?;
                if explicit.insert(seat, controller).is_some() {
                    return Err(format!("duplicate binding for {seat}"));
                }
            }
        }
        if specs.is_empty() {
            explicit.insert(
                "axis.commander".parse().unwrap(),
                Controller::Claude("haiku".into()),
            );
        }
        let seats = SeatId::all()
            .map(|seat| {
                (
                    seat,
                    explicit
                        .get(&seat)
                        .cloned()
                        .unwrap_or_else(|| wildcard.clone().unwrap_or_else(|| fallback.clone())),
                )
            })
            .collect();
        let config = Self {
            kind,
            seats,
            max_turns: 2,
            tool_calls: 40,
            session: None,
            run: None,
        };
        Ok(config)
    }
    pub fn validate(&self) -> Result<(), String> {
        if self.seats.len() != 10 || SeatId::all().any(|seat| !self.seats.contains_key(&seat)) {
            return Err("every campaign seat needs a binding".into());
        }
        if self.run.is_none() && self.claude_seats().count() > 2 {
            return Err("bounded launcher supports at most two Claude seats".into());
        }
        if let Some(run) = &self.run {
            if self.session.is_none() {
                return Err("run controls require durable sessions".into());
            }
            run.validate(&self.claude_seats().map(|(s, _)| s).collect::<Vec<_>>())?;
        }
        if let Some(limits) = &self.session {
            limits.validate()?;
            if !(1..=8192).contains(&self.max_turns) || !(1..=100_000).contains(&self.tool_calls) {
                return Err("durable budget exceeds safety bounds".into());
            }
        } else if !(1..=2).contains(&self.max_turns) || !(1..=40).contains(&self.tool_calls) {
            return Err("bounded budget requires 1..2 turns and 1..40 tool calls per seat".into());
        }
        for controller in self.seats.values() {
            match controller {
                Controller::Scripted(mode)
                    if !["legal_random", "pass_when_possible", "aggressive"]
                        .contains(&mode.as_str()) =>
                {
                    return Err("unknown scripted mode".into());
                }
                Controller::Scripted(mode)
                    if self.kind == GameKind::Cna && mode == "aggressive" =>
                {
                    return Err("CNA does not support scripted:aggressive".into());
                }
                Controller::Claude(model) => {
                    Controller::parse(&format!("claude:{model}"))?;
                }
                _ => {}
            }
        }
        Ok(())
    }
    pub fn claude_seats(&self) -> impl Iterator<Item = (SeatId, &str)> {
        self.seats
            .iter()
            .filter_map(|(seat, controller)| match controller {
                Controller::Claude(model) => Some((*seat, model.as_str())),
                _ => None,
            })
    }
    pub fn profile(&self) -> &'static str {
        match self.kind {
            GameKind::Sandbox => "sandbox-v1",
            GameKind::Cna => "cna-2021-dev",
        }
    }
}
#[derive(Debug)]
pub enum Command {
    Play(LaunchConfig),
    Replay(PathBuf),
    Resume(PathBuf),
    ResumeControlled {
        path: PathBuf,
        updates: ResumeUpdates,
    },
    Help,
}
#[derive(Clone, Debug, Default)]
pub struct ResumeUpdates {
    pub boundary: Option<cna_server::RunBoundary>,
    pub usd_shares: BTreeMap<SeatId, f64>,
    pub token_shares: BTreeMap<SeatId, u64>,
}
impl ResumeUpdates {
    pub fn apply(&self, old: &RunControl, seats: &[SeatId]) -> Result<RunControl, String> {
        let budget = match &old.budget {
            SpendBudget::ReportedUsd { global, .. }
                if !self.usd_shares.is_empty() && self.token_shares.is_empty() =>
            {
                SpendBudget::usd(*global, seats, self.usd_shares.clone())?
            }
            SpendBudget::Tokens { global, .. }
                if !self.token_shares.is_empty() && self.usd_shares.is_empty() =>
            {
                SpendBudget::tokens(*global, seats, self.token_shares.clone())?
            }
            _ if self.usd_shares.is_empty() && self.token_shares.is_empty() => old.budget.clone(),
            _ => return Err("rebalance unit must match the lifetime budget".into()),
        };
        let run = RunControl {
            boundary: self.boundary.unwrap_or(old.boundary),
            budget,
        };
        run.validate(seats)?;
        Ok(run)
    }
}

pub fn parse_args(args: &[String]) -> Result<Command, String> {
    let mut kind = GameKind::Sandbox;
    let mut specs = Vec::new();
    let mut turns = 2;
    let mut calls = 40;
    let mut session = None;
    let mut stop_turn = None;
    let mut stop_stage = None;
    let mut usd = None;
    let mut tokens = None;
    let mut usd_shares = BTreeMap::new();
    let mut token_shares = BTreeMap::new();
    let mut preset = false;
    let mut i = 0;
    if args == ["--help"] || args == ["-h"] {
        return Ok(Command::Help);
    }
    if args.first().map(String::as_str) == Some("--resume") {
        let path = args.get(1).ok_or("resume needs campaign database")?.into();
        if args.len() == 2 {
            return Ok(Command::Resume(path));
        }
        let mut updates = ResumeUpdates::default();
        let mut turn = None;
        let mut stage = None;
        let mut i = 2;
        while i < args.len() {
            let value = args.get(i + 1).ok_or("missing resume control value")?;
            match args[i].as_str() {
                "--stop-after-turn" => {
                    if turn
                        .replace(value.parse::<u16>().map_err(|_| "invalid game-turn")?)
                        .is_some()
                    {
                        return Err("duplicate boundary".into());
                    }
                }
                "--stop-after-opstage" => {
                    if stage
                        .replace(value.parse::<u8>().map_err(|_| "invalid OpStage")?)
                        .is_some()
                    {
                        return Err("duplicate OpStage".into());
                    }
                }
                "--seat-budget-usd" => parse_share(value, &mut updates.usd_shares)?,
                "--seat-budget-tokens" => parse_share(value, &mut updates.token_shares)?,
                _ => return Err(
                    "resume changes only boundary or explicit seat shares; lifetime caps remain"
                        .into(),
                ),
            }
            i += 2;
        }
        if stage.is_some() && turn.is_none() {
            return Err("OpStage requires game-turn".into());
        }
        updates.boundary = turn.map(|game_turn| cna_server::RunBoundary {
            game_turn,
            op_stage: stage,
        });
        if updates
            .boundary
            .is_some_and(|b| b.game_turn == 0 || b.op_stage.is_some_and(|s| !(1..=3).contains(&s)))
        {
            return Err("invalid resume boundary".into());
        }
        return Ok(Command::ResumeControlled { path, updates });
    }
    if args.first().map(String::as_str) == Some("--replay") {
        return if args.len() == 2 {
            Ok(Command::Replay(args[1].clone().into()))
        } else {
            Err("--replay takes exactly one database path".into())
        };
    }
    while i < args.len() {
        let flag = &args[i];
        if flag == "--long-lived" {
            session.get_or_insert_with(SessionLimits::default);
            i += 1;
            continue;
        }
        let value = args
            .get(i + 1)
            .ok_or_else(|| format!("missing value for {flag}"))?;
        match flag.as_str() {
            "--preset" => {
                if preset || value != "graziani-haiku" {
                    return Err("unknown or repeated preset".into());
                }
                preset = true;
                kind = GameKind::Cna;
                specs.push("*=claude:claude-haiku-4-5-20251001".into());
                session = Some(SessionLimits {
                    wall_seconds: 21_600,
                    ..SessionLimits::default()
                });
                turns = 8192;
                calls = 100_000;
            }
            "--stop-after-turn" => {
                if stop_turn
                    .replace(value.parse::<u16>().map_err(|_| "invalid game-turn")?)
                    .is_some()
                {
                    return Err("duplicate game-turn boundary".into());
                }
            }
            "--stop-after-opstage" => {
                if stop_stage
                    .replace(value.parse::<u8>().map_err(|_| "invalid OpStage")?)
                    .is_some()
                {
                    return Err("duplicate OpStage boundary".into());
                }
            }
            "--budget-usd" => {
                if usd
                    .replace(value.parse::<f64>().map_err(|_| "invalid USD cap")?)
                    .is_some()
                {
                    return Err("duplicate USD cap".into());
                }
            }
            "--budget-tokens" => {
                if tokens
                    .replace(value.parse::<u64>().map_err(|_| "invalid token cap")?)
                    .is_some()
                {
                    return Err("duplicate token cap".into());
                }
            }
            "--seat-budget-usd" => parse_share(value, &mut usd_shares)?,
            "--seat-budget-tokens" => parse_share(value, &mut token_shares)?,
            "--kind" => {
                kind = match value.as_str() {
                    "sandbox" => GameKind::Sandbox,
                    "cna" => GameKind::Cna,
                    _ => return Err("--kind must be sandbox or cna".into()),
                }
            }
            "--seat" => specs.push(value.clone()),
            "--turns" => turns = value.parse().map_err(|_| "invalid --turns")?,
            "--tool-calls" => calls = value.parse().map_err(|_| "invalid --tool-calls")?,
            "--wall-seconds" | "--turn-timeout" | "--context-tokens" | "--recoveries" => {
                let limits = session.get_or_insert_with(SessionLimits::default);
                match flag.as_str() {
                    "--wall-seconds" => {
                        limits.wall_seconds = value.parse().map_err(|_| "invalid wall limit")?
                    }
                    "--turn-timeout" => {
                        limits.turn_seconds = value.parse().map_err(|_| "invalid turn limit")?
                    }
                    "--context-tokens" => {
                        limits.context_tokens =
                            value.parse().map_err(|_| "invalid context limit")?
                    }
                    _ => limits.recoveries = value.parse().map_err(|_| "invalid recovery limit")?,
                }
            }
            _ => return Err(format!("unknown option {flag}")),
        }
        i += 2;
    }
    let mut config = LaunchConfig::resolve_bindings(kind, &specs)?;
    config.max_turns = turns;
    config.tool_calls = calls;
    config.session = session;
    if preset
        || stop_turn.is_some()
        || stop_stage.is_some()
        || usd.is_some()
        || tokens.is_some()
        || !usd_shares.is_empty()
        || !token_shares.is_empty()
    {
        let boundary = cna_server::RunBoundary {
            game_turn: stop_turn.ok_or("run controls require --stop-after-turn")?,
            op_stage: stop_stage,
        };
        let seats = config.claude_seats().map(|(s, _)| s).collect::<Vec<_>>();
        let budget = match (usd, tokens) {
            (Some(cap), None) if token_shares.is_empty() => {
                SpendBudget::usd(cap, &seats, usd_shares)?
            }
            (None, Some(cap)) if usd_shares.is_empty() => {
                SpendBudget::tokens(cap, &seats, token_shares)?
            }
            _ => return Err("choose exactly one reported-USD or token cap".into()),
        };
        config.run = Some(RunControl { boundary, budget });
    }
    config.validate()?;
    if preset
        && (config.kind != GameKind::Cna
            || config.claude_seats().count() != 10
            || config
                .claude_seats()
                .any(|(_, m)| m != "claude-haiku-4-5-20251001"))
    {
        return Err("graziani-haiku preset requires all ten Haiku seats and CNA".into());
    }
    Ok(Command::Play(config))
}

fn parse_share<T: std::str::FromStr>(
    value: &str,
    shares: &mut BTreeMap<SeatId, T>,
) -> Result<(), String> {
    let (seat, value) = value
        .split_once('=')
        .ok_or("seat budget requires SEAT=AMOUNT")?;
    let seat = seat.parse::<SeatId>().map_err(|e| e.to_string())?;
    let value = value.parse::<T>().map_err(|_| "invalid seat budget")?;
    if shares.insert(seat, value).is_some() {
        return Err("duplicate seat allocation".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn args(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).into()).collect()
    }
    #[test]
    fn preset_requires_a_limit_and_durable_controlled_resume() {
        let Command::Play(config) = parse_args(&args(&[
            "--preset",
            "graziani-haiku",
            "--stop-after-turn",
            "1",
            "--budget-usd",
            "10",
        ]))
        .unwrap() else {
            panic!()
        };
        assert_eq!(config.claude_seats().count(), 10);
        assert!(config.session.is_some());
        assert_eq!(config.run.unwrap().boundary.game_turn, 1);
        for bad in [
            args(&["--preset", "graziani-haiku"]),
            args(&[
                "--preset",
                "graziani-haiku",
                "--stop-after-turn",
                "0",
                "--budget-usd",
                "10",
            ]),
            args(&[
                "--preset",
                "graziani-haiku",
                "--stop-after-turn",
                "1",
                "--budget-usd",
                "NaN",
            ]),
            args(&["--resume", "a.sqlite", "--budget-usd", "99"]),
        ] {
            assert!(parse_args(&bad).is_err());
        }
        assert!(matches!(
            parse_args(&args(&["--resume", "a.sqlite", "--stop-after-turn", "2"])).unwrap(),
            Command::ResumeControlled { .. }
        ));
    }
    #[test]
    fn explicit_bindings_override_wildcard_in_either_order() {
        let one = LaunchConfig::resolve(
            GameKind::Cna,
            &args(&["axis.commander=claude:haiku", "*=scripted:legal_random"]),
        )
        .unwrap();
        let two = LaunchConfig::resolve(
            GameKind::Cna,
            &args(&["*=scripted:legal_random", "axis.commander=claude:haiku"]),
        )
        .unwrap();
        assert_eq!(one.seats, two.seats);
        assert_eq!(one.claude_seats().count(), 1);
        assert_eq!(one.profile(), "cna-2021-dev");
    }
    #[test]
    fn bad_bindings_and_unconfined_clis_are_rejected() {
        for spec in [
            "axis.navy=human",
            "axis.commander=codex:cheap",
            "*=claude:haiku",
            "*=scripted:aggressive",
            "axis.commander=claude:--help",
        ] {
            assert!(
                LaunchConfig::resolve(GameKind::Cna, &args(&[spec])).is_err(),
                "{spec}"
            );
        }
        assert!(
            LaunchConfig::resolve(
                GameKind::Cna,
                &args(&["axis.commander=human", "axis.commander=human"])
            )
            .is_err()
        );
        assert!(LaunchConfig::resolve(GameKind::Cna, &args(&["*=human", "*=human"])).is_err());
    }
    #[test]
    fn parsing_distinguishes_no_paid_sessions_and_bounded_opt_in() {
        let Command::Play(config) = parse_args(&args(&[
            "--kind",
            "cna",
            "--seat",
            "*=scripted:legal_random",
            "--turns",
            "1",
            "--tool-calls",
            "8",
        ]))
        .unwrap() else {
            panic!()
        };
        assert_eq!(config.claude_seats().count(), 0);
        assert_eq!(config.max_turns, 1);
        assert_eq!(config.tool_calls, 8);
        assert!(parse_args(&args(&["--turns", "3"])).is_err());
        assert!(parse_args(&args(&["--replay", "a.sqlite", "--kind", "cna"])).is_err());
        assert!(parse_args(&args(&["--seat"])).is_err());
    }
}

/// Lifetime bounds stored with the campaign; resume never resets them.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionLimits {
    pub wall_seconds: u64,
    pub turn_seconds: u64,
    pub context_tokens: u64,
    pub recoveries: u32,
}
impl Default for SessionLimits {
    fn default() -> Self {
        Self {
            wall_seconds: 150,
            turn_seconds: 60,
            context_tokens: 100_000,
            recoveries: 2,
        }
    }
}
impl SessionLimits {
    pub fn validate(&self) -> Result<(), String> {
        if !(1..=21_600).contains(&self.wall_seconds)
            || !(1..=1200).contains(&self.turn_seconds)
            || self.turn_seconds > self.wall_seconds
            || !(16_000..=200_000).contains(&self.context_tokens)
            || self.recoveries > 8
        {
            return Err("invalid durable wall/turn/context/recovery limits".into());
        }
        Ok(())
    }
}
