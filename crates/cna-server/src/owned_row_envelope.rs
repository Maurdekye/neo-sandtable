//! PRIVATE owned row admission. Legacy Campaign/Factory paths do not use it.
//! m80deb: numeric policy is applied to borrowed raw TEXT before retained copies.
//! D1032 requires the stock-Windows build/process binding in4586036F at STRICT.
use crate::owned_writer::RowFailure;
use rusqlite::{Connection, Row, types::ValueRef};
use serde::{Deserialize, Deserializer, de::{self, Visitor}};
use std::fmt;

pub(crate) const TITLE_UTF8_MAX: usize = 256;
pub(crate) const TITLE_ESCAPED_MAX: usize = 512;
pub(crate) const STATUS_MAX: usize = 8192;
pub(crate) const DATABASE_PATH_MAX: usize = 1032;
pub(crate) const STATE_HASH_MAX: usize = 64;
pub(crate) const META_DEV_MAX: usize = 1092 + TITLE_ESCAPED_MAX;
pub(crate) const META_FULL_MAX: usize = 1093 + TITLE_ESCAPED_MAX;
pub(crate) const META_SANDBOX_MAX: usize = 1089 + TITLE_ESCAPED_MAX;
pub(crate) const PINS_DEV_MAX: usize = 198;
pub(crate) const PINS_FULL_MAX: usize = 199;
pub(crate) const PINS_SANDBOX_MAX: usize = 196;
pub(crate) const WRITER_ROW_BYTES: usize = META_FULL_MAX + PINS_FULL_MAX
    + STATE_HASH_MAX + STATUS_MAX + DATABASE_PATH_MAX;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TitleRefusal { Bytes { actual: usize }, Control { byte_index: usize } }

pub(crate) fn title(original: &str) -> Result<(), TitleRefusal> {
    if original.len() > TITLE_UTF8_MAX { return Err(TitleRefusal::Bytes { actual: original.len() }); }
    if let Some((byte_index, _)) = original.char_indices().find(|(_, c)| *c <= '\u{1f}') {
        return Err(TitleRefusal::Control { byte_index });
    }
    Ok(())
}

struct CheckedTitle(Result<(), TitleRefusal>);
impl<'de> Deserialize<'de> for CheckedTitle {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct TitleVisitor;
        impl Visitor<'_> for TitleVisitor {
            type Value = CheckedTitle;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("a campaign title") }
            fn visit_str<E: de::Error>(self, text: &str) -> Result<CheckedTitle, E> {
                // No retained String/metadata object before this check. Escaped
                // JSON still works through serde's temporary decode scratch.
                Ok(CheckedTitle(title(text)))
            }
        }
        deserializer.deserialize_str(TitleVisitor)
    }
}
#[derive(Deserialize)]
struct TitleOnly { title: CheckedTitle }

#[derive(Clone, Copy, Debug)]
enum Profile { Dev, Full, Sandbox }

impl<'de> Deserialize<'de> for Profile {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ProfileVisitor;
        impl Visitor<'_> for ProfileVisitor {
            type Value = Profile;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result { f.write_str("a supported campaign profile") }
            fn visit_str<E: de::Error>(self, text: &str) -> Result<Profile, E> {
                match text {
                    cna_rules::PROFILE_DEV => Ok(Profile::Dev),
                    cna_rules::PROFILE_FULL => Ok(Profile::Full),
                    cna_sandbox::PROFILE_ID => Ok(Profile::Sandbox),
                    _ => Err(E::custom("unsupported campaign profile")),
                }
            }
        }
        deserializer.deserialize_str(ProfileVisitor)
    }
}

#[derive(Deserialize)]
struct ProfileOnly { rules_profile: Profile }

fn length(actual: usize, available: usize) -> Result<(), RowFailure> {
    if actual > available { Err(RowFailure::Capacity { required: actual, available }) } else { Ok(()) }
}

pub(crate) fn path(original: Option<&str>) -> Result<(), RowFailure> {
    let original = original.filter(|path| !path.is_empty()).ok_or(RowFailure::MissingConnectionPath)?;
    length(original.len(), DATABASE_PATH_MAX)
}

/// Only a tiny profile enum is decoded before its per-profile checks. No owned
/// Pins/metadata String/Vec or StoredWriterRow is produced here. Parser scratch
/// and raw SQLite conversion backing are NOT claimed allocation-free/exempt.
pub(crate) fn fields(fields: [&[u8]; 5]) -> Result<(), RowFailure> {
    length(fields[0].len(), META_FULL_MAX)?;
    length(fields[1].len(), PINS_FULL_MAX)?;
    length(fields[2].len(), STATE_HASH_MAX)?;
    length(fields[3].len(), STATUS_MAX)?;
    length(fields[4].len(), DATABASE_PATH_MAX)?;
    if fields[4].is_empty() { return Err(RowFailure::MissingConnectionPath); }
    let profile: ProfileOnly = serde_json::from_slice(fields[1]).map_err(RowFailure::Json)?;
    let (meta, pins) = match profile.rules_profile {
        Profile::Dev => (META_DEV_MAX, PINS_DEV_MAX),
        Profile::Full => (META_FULL_MAX, PINS_FULL_MAX),
        Profile::Sandbox => (META_SANDBOX_MAX, PINS_SANDBOX_MAX),
    };
    length(fields[0].len(), meta)?;
    length(fields[1].len(), pins)?;
    let admitted: TitleOnly = serde_json::from_slice(fields[0]).map_err(RowFailure::Json)?;
    admitted.title.0.map_err(RowFailure::Title)
}

fn text<'r>(row: &'r Row<'_>, index: usize) -> Result<&'r [u8], RowFailure> {
    match row.get_ref(index)? {
        ValueRef::Text(original) => Ok(original),
        other => Err(RowFailure::ColumnType { column: index, actual: other.data_type() }),
    }
}

pub(crate) type RecoveryFields = (String, String, u64, String, String, String);

/// Refuse oversized input on READ_ONLY before even opening the writable
/// connection: that open could otherwise perform SQLite journal recovery.
/// No decoded metadata/Pins or owned raw fields are retained by this check.
pub(crate) fn check_recovery(db: &Connection) -> Result<(), RowFailure> {
    let mut statement = db.prepare("SELECT meta,pins,state_hash,status FROM campaign WHERE id=1")?;
    let mut rows = statement.query([])?;
    let row = rows.next()?.ok_or(rusqlite::Error::QueryReturnedNoRows)?;
    let filename = db.path().ok_or(RowFailure::MissingConnectionPath)?;
    fields([text(row,0)?,text(row,1)?,text(row,2)?,text(row,3)?,filename.as_bytes()])
}

/// SAME live row: checks all raw envelopes before any get::<String> below.
/// The caller opens this connection before configuring WAL/schema/migrations.
pub(crate) fn recovery_fields(db: &Connection) -> Result<RecoveryFields, RowFailure> {
    let mut statement = db.prepare("SELECT meta, pins, revision, rng, state_hash, status FROM campaign WHERE id=1")?;
    let mut rows = statement.query([])?;
    let row = rows.next()?.ok_or(rusqlite::Error::QueryReturnedNoRows)?;
    let filename = db.path().ok_or(RowFailure::MissingConnectionPath)?;
    fields([text(row,0)?,text(row,1)?,text(row,4)?,text(row,5)?,filename.as_bytes()])?;
    Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?,row.get(4)?,row.get(5)?))
}

/// The initial READ_ONLY discriminator has the same raw size checks, without
/// retaining unrelated RNG/state copies or touching the legacy reader.
pub(crate) fn recovery_identity(db: &Connection)
    -> Result<(cna_protocol::CampaignMeta, crate::Pins), RowFailure>
{
    let mut statement = db.prepare("SELECT meta,pins,state_hash,status FROM campaign WHERE id=1")?;
    let mut rows = statement.query([])?;
    let row = rows.next()?.ok_or(rusqlite::Error::QueryReturnedNoRows)?;
    let filename = db.path().ok_or(RowFailure::MissingConnectionPath)?;
    let meta = text(row,0)?;
    let pins = text(row,1)?;
    fields([meta,pins,text(row,2)?,text(row,3)?,filename.as_bytes()])?;
    Ok((serde_json::from_slice(meta).map_err(RowFailure::Json)?,
        serde_json::from_slice(pins).map_err(RowFailure::Json)?))
}

#[cfg(test)]
#[path = "owned_row_envelope_controls.rs"]
mod controls;
