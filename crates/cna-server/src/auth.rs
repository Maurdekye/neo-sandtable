//! Process-local capabilities. Restricted credentials are bound to one campaign and perspective.
//! Seat confinement must keep these secrets out of untrusted filesystem/environment access.
use cna_core::{ids::SeatId, visibility::Perspective};
use cna_protocol::Side;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::RwLock,
};

#[derive(Clone)]
pub(crate) struct Grant {
    pub perspective: Perspective,
    pub campaign_id: Option<String>,
}
impl Grant {
    pub fn operator(&self) -> bool {
        self.perspective == Perspective::Operator
    }
    pub fn campaign(&self, id: &str) -> bool {
        self.operator() || self.campaign_id.as_deref() == Some(id)
    }
    pub fn perspective(&self, p: Perspective) -> bool {
        match (self.perspective, p) {
            (Perspective::Operator, _) => true,
            (Perspective::Side(a), Perspective::Side(b)) => a == b,
            (Perspective::Side(a), Perspective::Seat(b)) => a == b.side,
            (Perspective::Seat(a), Perspective::Seat(b)) => a == b,
            _ => false,
        }
    }
    pub fn seat(&self, seat: SeatId, write: bool) -> bool {
        self.operator()
            || match self.perspective {
                Perspective::Side(side) => !write && side == seat.side,
                Perspective::Seat(own) => own == seat,
                Perspective::Operator => true,
            }
    }
}
#[derive(Clone, Serialize)]
pub struct CampaignCapabilities {
    pub sides: BTreeMap<Side, String>,
    pub seats: BTreeMap<SeatId, String>,
}
#[derive(Serialize)]
struct Credentials<'a> {
    operator: &'a str,
    campaigns: &'a BTreeMap<String, CampaignCapabilities>,
}
struct Store {
    grants: BTreeMap<String, Grant>,
    campaigns: BTreeMap<String, CampaignCapabilities>,
    export: Option<PathBuf>,
}
pub(crate) struct Capabilities {
    operator: String,
    store: RwLock<Store>,
}
fn random_token() -> String {
    // Two OS-random v4 UUIDs supply 244 unpredictable bits, independent of game RNG.
    format!(
        "{}{}",
        uuid::Uuid::new_v4().simple(),
        uuid::Uuid::new_v4().simple()
    )
}
fn token_hash(token: &str) -> String {
    format!("{:x}", Sha256::digest(token.as_bytes()))
}
impl Capabilities {
    pub fn new() -> Self {
        let operator = random_token();
        let grants = BTreeMap::from([(
            token_hash(&operator),
            Grant {
                perspective: Perspective::Operator,
                campaign_id: None,
            },
        )]);
        Self {
            operator,
            store: RwLock::new(Store {
                grants,
                campaigns: BTreeMap::new(),
                export: None,
            }),
        }
    }
    pub fn operator_token(&self) -> String {
        self.operator.clone()
    }
    pub fn authenticate(&self, token: &str) -> Option<Grant> {
        if token.len() != 64 || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
            return None;
        }
        self.store
            .read()
            .ok()?
            .grants
            .get(&token_hash(token))
            .cloned()
    }
    pub fn register(&self, id: &str) {
        let mut store = self.store.write().expect("capability registry");
        if store.campaigns.contains_key(id) {
            return;
        }
        let mut tokens = CampaignCapabilities {
            sides: BTreeMap::new(),
            seats: BTreeMap::new(),
        };
        for p in Perspective::all().filter(|p| *p != Perspective::Operator) {
            let token = random_token();
            store.grants.insert(
                token_hash(&token),
                Grant {
                    perspective: p,
                    campaign_id: Some(id.into()),
                },
            );
            match p {
                Perspective::Side(side) => {
                    tokens.sides.insert(side, token);
                }
                Perspective::Seat(seat) => {
                    tokens.seats.insert(seat, token);
                }
                Perspective::Operator => unreachable!(),
            }
        }
        store.campaigns.insert(id.into(), tokens);
        if let Some(path) = &store.export
            && let Err(error) = export(path, &self.operator, &store.campaigns)
        {
            // Never log credentials, request headers or WebSocket query strings.
            tracing::error!(%error,"could not refresh operator credentials file");
        }
    }
    pub fn campaign_tokens(&self, id: &str) -> Option<CampaignCapabilities> {
        self.store.read().ok()?.campaigns.get(id).cloned()
    }
    pub fn write_credentials(&self, path: &Path) -> Result<(), std::io::Error> {
        let mut store = self.store.write().expect("capability registry");
        export(path, &self.operator, &store.campaigns)?;
        store.export = Some(path.to_owned());
        Ok(())
    }
}
fn export(
    path: &Path,
    operator: &str,
    campaigns: &BTreeMap<String, CampaignCapabilities>,
) -> Result<(), std::io::Error> {
    let bytes = serde_json::to_vec_pretty(&Credentials {
        operator,
        campaigns,
    })?;
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    // Atomic replacement: readers see the complete old or new credential document.
    let temp = parent.join(format!(".capabilities-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temp)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        // std::fs::rename atomically replaces an existing file on supported platforms.
        fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}
