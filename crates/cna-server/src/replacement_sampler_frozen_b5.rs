//! Frozen own b5 scripted.rs sample/Generator/ResourcePreview/answer bodies.
//! Only private observation insertions and answer visibility differ; inverse pinned.
use super::*;

fn sample(rng: &mut ChaCha8Rng, count: u128) -> Result<u64, Error> {
    observe(Phase::Sample(count), rng);
    if count == 0 || count > (1u128 << 64) {
        return Err(Error::Invalid("empty or invalid action domain".into()));
    }
    if count == (1u128 << 64) {
        return Ok(rng.next_u64());
    }
    let n = count as u64;
    let threshold = n.wrapping_neg() % n;
    loop {
        let value = rng.next_u64();
        if value >= threshold {
            return Ok(value % n);
        }
    }
}

struct Generator<'a> {
    request: &'a DecisionRequest,
    rng: ChaCha8Rng,
    nodes_left: u32,
    candidates: &'a dyn Candidates,
    attempt: u32,
}
impl Generator<'_> {
    fn action(&mut self, schema: &ActionSchema, depth: u32) -> Result<Value, Error> {
        observe(Phase::Generate, &self.rng);
        if depth > 32 || self.nodes_left == 0 {
            return Err(Error::Invalid("action generation budget exhausted".into()));
        }
        self.nodes_left -= 1;
        Ok(match schema {
            ActionSchema::Choice { options } => options
                [sample(&mut self.rng, options.len() as u128)? as usize]
                .id
                .clone()
                .into(),
            ActionSchema::Integer { min, max } => {
                if min > max {
                    return Err(Error::Invalid("inverted integer domain".into()));
                }
                let offset = sample(
                    &mut self.rng,
                    (i128::from(*max) - i128::from(*min) + 1) as u128,
                )?;
                Value::from((i128::from(*min) + i128::from(offset)) as i64)
            }
            ActionSchema::Bool => Value::Bool(sample(&mut self.rng, 2)? == 1),
            ActionSchema::Unit { among } => among
                [sample(&mut self.rng, among.len() as u128)? as usize]
                .as_str()
                .into(),
            ActionSchema::Hex { among: Some(among) } => among
                [sample(&mut self.rng, among.len() as u128)? as usize]
                .as_str()
                .into(),
            ActionSchema::Hex { among: None }
            | ActionSchema::Path { .. }
            | ActionSchema::Text { .. } => self
                .candidates
                .candidate(self.request, schema, self.attempt)
                .ok_or_else(|| {
                    Error::Invalid("ruleset candidate hook required for hex/path actions".into())
                })?,
            ActionSchema::Record { fields } => {
                let mut object = Map::new();
                for field in fields {
                    if !field.optional || sample(&mut self.rng, 2)? == 1 {
                        object.insert(field.name.clone(), self.action(&field.schema, depth + 1)?);
                    }
                }
                Value::Object(object)
            }
            ActionSchema::List { item, min, max } => {
                if min > max {
                    return Err(Error::Invalid("inverted list domain".into()));
                }
                let length = u64::from(*min) + sample(&mut self.rng, u128::from(max - min) + 1)?;
                if length > u64::from(self.nodes_left) {
                    return Err(Error::Invalid("action generation budget exhausted".into()));
                }
                let mut list = Vec::new();
                for _ in 0..length {
                    list.push(self.action(item, depth + 1)?);
                }
                Value::Array(list)
            }
        })
    }
}

#[derive(Debug, PartialEq, Eq)]
enum PreviewStop {
    Budget,
    Legacy,
}

/// Count the exact seeded traversal without constructing values or calling hooks.
/// The real generator retains its original RNG and counters. Invalid domains and
/// reached opaque leaves defer to it, preserving their existing failure behavior.
struct ResourcePreview {
    rng: ChaCha8Rng,
    nodes_left: u32,
}
impl ResourcePreview {
    fn discard_sample(&mut self, count: u128) -> Result<(), PreviewStop> {
        sample(&mut self.rng, count)
            .map(|_| ())
            .map_err(|_| PreviewStop::Legacy)
    }

    fn action(&mut self, schema: &ActionSchema, depth: u32) -> Result<(), PreviewStop> {
        observe(Phase::Preview, &self.rng);
        if depth > 32 || self.nodes_left == 0 {
            return Err(PreviewStop::Budget);
        }
        self.nodes_left -= 1;
        match schema {
            ActionSchema::Choice { options } => self.discard_sample(options.len() as u128),
            ActionSchema::Integer { min, max } => {
                if min > max {
                    return Err(PreviewStop::Legacy);
                }
                self.discard_sample((i128::from(*max) - i128::from(*min) + 1) as u128)
            }
            ActionSchema::Bool => self.discard_sample(2),
            ActionSchema::Unit { among } => self.discard_sample(among.len() as u128),
            ActionSchema::Hex { among: Some(among) } => self.discard_sample(among.len() as u128),
            ActionSchema::Hex { among: None }
            | ActionSchema::Path { .. }
            | ActionSchema::Text { .. } => Err(PreviewStop::Legacy),
            ActionSchema::Record { fields } => {
                for field in fields {
                    if !field.optional
                        || sample(&mut self.rng, 2).map_err(|_| PreviewStop::Legacy)? == 1
                    {
                        self.action(&field.schema, depth + 1)?;
                    }
                }
                Ok(())
            }
            ActionSchema::List { item, min, max } => {
                if min > max {
                    return Err(PreviewStop::Legacy);
                }
                let length = u64::from(*min)
                    + sample(&mut self.rng, u128::from(max - min) + 1)
                        .map_err(|_| PreviewStop::Legacy)?;
                if length > u64::from(self.nodes_left) {
                    return Err(PreviewStop::Budget);
                }
                for _ in 0..length {
                    self.action(item, depth + 1)?;
                }
                Ok(())
            }
        }
    }
}

fn requires_candidates(schema: &ActionSchema) -> bool {
    match schema {
        ActionSchema::Hex { among: None }
        | ActionSchema::Path { .. }
        | ActionSchema::Text { .. } => true,
        ActionSchema::Record { fields } => fields
            .iter()
            .any(|field| requires_candidates(&field.schema)),
        ActionSchema::List { item, max, .. } => *max > 0 && requires_candidates(item),
        _ => false,
    }
}

fn unavailable_domain(request: &DecisionRequest, candidates: &dyn Candidates) -> bool {
    !candidates.has_unenumerated_domains() && requires_candidates(&request.space.schema)
}

pub(super) fn answer(
    request: &DecisionRequest,
    epoch: u64,
    mode: Mode,
    attempt: u32,
    candidates: &dyn Candidates,
) -> Result<DecisionResponse, Error> {
    let seed: [u8; 32] = Sha256::digest(serde_json::to_vec(&(request, epoch, attempt))?).into();
    let mut generator = Generator {
        request,
        rng: ChaCha8Rng::from_seed(seed),
        nodes_left: 4096,
        candidates,
        attempt,
    };
    observe(Phase::Begin, &generator.rng);
    let unavailable = unavailable_domain(request, candidates);
    if unavailable && request.space.pass.is_none() {
        return Err(Error::Invalid(format!(
            "scripted action domain unavailable for {}: ruleset candidate hook required and no declared pass",
            request.kind
        )));
    }
    // A declared pass is this baseline's policy for an unavailable domain. It is
    // selected before generation, never substituted after an execution failure.
    let pass = request.space.pass.is_some()
        && (unavailable || mode == Mode::PassWhenPossible || sample(&mut generator.rng, 5)? == 0);
    let budget_unavailable = !pass
        && ResourcePreview {
            rng: generator.rng.clone(),
            nodes_left: generator.nodes_left,
        }
        .action(&request.space.schema, 0)
            == Err(PreviewStop::Budget);
    if budget_unavailable && request.space.pass.is_none() {
        return Err(Error::Invalid(format!(
            "scripted action domain unavailable for {}: generation budget would be exhausted and no declared pass",
            request.kind
        )));
    }
    // Resource policy is decided before real generation, not after an error.
    let action = if pass || budget_unavailable {
        Value::Null
    } else {
        generator.action(&request.space.schema, 0)?
    };
    observe(Phase::End, &generator.rng);
    Ok(DecisionResponse {
        decision_id: request.id.clone(),
        seat: request.seat,
        controller_epoch: epoch,
        decision_revision: request.revision,
        idempotency_key: format!(
            "scripted:{}:{epoch}:{}:{attempt}",
            request.id, request.revision
        ),
        action,
        public_explanation: None,
    })
}


