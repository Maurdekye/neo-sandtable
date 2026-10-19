//! Existing campaign writer diagnostic ownership, m7cb5c6de0a9b.
//! Full errors MOVE here before publication; Operation entries retain a Copy tag.
use crate::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FatalClass {
    Storage,
    Json,
    Recovery,
    Other,
}
impl FatalClass {
    pub(crate) fn of(error: &Error) -> Self {
        match error {
            Error::Storage(_) => Self::Storage,
            Error::Json(_) => Self::Json,
            Error::Recovery(_) => Self::Recovery,
            _ => Self::Other,
        }
    }
}

/// Two fixed campaign-scoped slots in the already existing writer result path.
/// No operation-indexed map, diagnostic queue, clone or allocation on transfer.
#[derive(Default)]
pub(crate) struct WriterFatals {
    primary: Option<Error>,
    secondary: Option<Error>,
    secondary_reported: bool,
    closed_cleanup: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Recorded {
    Primary,
    Secondary,
    Logged,
}
impl WriterFatals {
    pub(crate) fn record(&mut self, error: Error) -> Recorded {
        if self.closed_cleanup {
            // One log for this occurrence, then DROP. No retained late queue.
            tracing::error!(%error, "campaign writer closed cleanup failure");
            Recorded::Logged
        } else if self.primary.is_none() {
            self.primary = Some(error);
            Recorded::Primary
        } else if self.secondary.is_none() {
            self.secondary = Some(error);
            Recorded::Secondary
        } else {
            tracing::error!(%error, "campaign writer later fatal failure");
            Recorded::Logged
        }
    }
    pub(crate) fn primary(&self) -> Option<&Error> {
        self.primary.as_ref()
    }
    pub(crate) fn secondary(&self) -> Option<&Error> {
        self.secondary.as_ref()
    }
    pub(crate) fn report_secondary(&mut self) {
        if !self.secondary_reported {
            if let (Some(primary), Some(secondary)) = (&self.primary, &self.secondary) {
                tracing::error!(%primary, %secondary, "campaign writer secondary failure");
                self.secondary_reported = true;
            }
        }
    }
    pub(crate) fn enter_closed_cleanup(&mut self) {
        self.closed_cleanup = true;
    }
    pub(crate) fn into_result(self) -> Result<(), Error> {
        match self.primary {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(error: &Error) -> &String {
        match error {
            Error::Recovery(text) => text,
            _ => panic!("expected test diagnostic"),
        }
    }
    #[test]
    fn backend_before_publication_is_primary_and_both_errors_move_without_clone() {
        let backend = String::from("backend");
        let backend_address = backend.as_ptr();
        let publication = String::from("publication");
        let publication_address = publication.as_ptr();
        let mut writer = WriterFatals::default();
        assert_eq!(writer.record(Error::Recovery(backend)), Recorded::Primary);
        assert_eq!(
            writer.record(Error::Recovery(publication)),
            Recorded::Secondary
        );
        assert_eq!(text(writer.primary().unwrap()).as_ptr(), backend_address);
        assert_eq!(
            text(writer.secondary().unwrap()).as_ptr(),
            publication_address
        );
        assert_eq!(text(writer.primary().unwrap()), "backend");
        assert_eq!(text(writer.secondary().unwrap()), "publication");
        let primary = writer.into_result().unwrap_err();
        assert_eq!(text(&primary).as_ptr(), backend_address);
    }
    #[test]
    fn publication_without_earlier_backend_is_primary() {
        let mut writer = WriterFatals::default();
        assert_eq!(
            writer.record(Error::Recovery("publication".into())),
            Recorded::Primary
        );
        assert!(writer.secondary().is_none());
        assert_eq!(text(&writer.into_result().unwrap_err()), "publication");
    }
    #[test]
    fn later_and_closed_cleanup_errors_never_displace_primary_or_secondary() {
        let mut writer = WriterFatals::default();
        writer.record(Error::Recovery("backend".into()));
        writer.record(Error::Recovery("publication".into()));
        assert_eq!(
            writer.record(Error::Recovery("third".into())),
            Recorded::Logged
        );
        writer.enter_closed_cleanup();
        assert_eq!(
            writer.record(Error::Recovery("cleanup".into())),
            Recorded::Logged
        );
        assert_eq!(text(writer.primary().unwrap()), "backend");
        assert_eq!(text(writer.secondary().unwrap()), "publication");
        assert_eq!(text(&writer.into_result().unwrap_err()), "backend");
    }
    #[test]
    fn closed_cleanup_does_not_fill_an_unused_secondary_slot() {
        let mut writer = WriterFatals::default();
        writer.record(Error::Recovery("primary".into()));
        writer.enter_closed_cleanup();
        assert_eq!(
            writer.record(Error::Recovery("cleanup".into())),
            Recorded::Logged
        );
        assert!(writer.secondary().is_none());
        assert_eq!(text(&writer.into_result().unwrap_err()), "primary");
    }
    #[test]
    fn fatal_class_is_fixed_copy_without_diagnostic_payload() {
        fn copy_type<T: Copy>() {}
        copy_type::<FatalClass>();
        let diagnostic = Error::Json(serde_json::from_str::<serde_json::Value>("{").unwrap_err());
        assert_eq!(FatalClass::of(&diagnostic), FatalClass::Json);
        assert_eq!(
            FatalClass::of(&Error::Storage(rusqlite::Error::InvalidQuery)),
            FatalClass::Storage
        );
        assert_eq!(
            FatalClass::of(&Error::Recovery("body stays in writer".into())),
            FatalClass::Recovery
        );
        assert_eq!(FatalClass::of(&Error::NotRunning), FatalClass::Other);
    }
}
