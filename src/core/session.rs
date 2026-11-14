//! The session contract: a resource one case keeps alive across its steps.
//!
//! A step that needs a browser page, a device or a connection asks [`Sessions`]
//! for it on first use. Every later step and every observation of the same case
//! receives the same value, and the runner closes it when the case ends, whether
//! the case passed, failed, errored, timed out or stopped early.
//!
//! The engine never names a concrete resource: a plugin declares its own type,
//! implements [`Session`] and chooses the kind it is stored under. The runner
//! gives each case its own [`Sessions`], so cases that run at the same time with
//! `--jobs` cannot share one.

use std::any::Any;

use crate::core::registry::is_allowed_name_character;

/// Makes a session downcast to the concrete type that created it.
///
/// It has a blanket implementation, so a session only implements [`Session`].
/// The trait is public because [`Session`] requires it, not because a plugin
/// should implement it directly.
pub trait AsAny: Any {
    /// Returns this value as [`Any`], which is what the downcast needs.
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

impl<T: Any> AsAny for T {
    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

/// A resource a case keeps alive across its steps.
///
/// The runner calls [`Self::close`] exactly once when the case ends. An
/// implementation must release everything it holds, including a connection or a
/// child process, and return a message the user can act on if it cannot.
///
/// `Send` is required because a case may run on a worker thread. A session
/// belongs to one case; it is never shared with another case, even under
/// `--jobs`.
pub trait Session: Send + AsAny {
    /// Releases the resource.
    ///
    /// The message is reported as a close error. The runner has already decided
    /// the case's own result, so a close failure never hides it.
    fn close(&mut self) -> Result<(), String>;
}

/// The sessions of one case, keyed by the kind the plugin chose.
///
/// A kind follows the step name rule: lowercase letters, digits, `.`, `_` and
/// `-`. It is a plugin's own namespace, so a browser page and a network capture
/// can share one case without naming each other.
#[derive(Default)]
pub struct Sessions {
    /// In creation order; [`Self::close_all`] pops from the end.
    entries: Vec<SessionEntry>,
}

/// One open session and the kind it is stored under.
struct SessionEntry {
    kind: &'static str,
    session: Box<dyn Session>,
}

impl Sessions {
    /// Creates an empty set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the session of `kind`, creating it with `create` on first use.
    ///
    /// The generic type is the concrete session type. A kind that is already
    /// open with another type is an error, not a panic: one kind holds one
    /// concrete type for the whole case.
    ///
    /// The factory runs at most once per case. If it returns an error, no slot
    /// is left behind, so a later step may try again.
    pub fn get_or_create<T>(
        &mut self,
        kind: &'static str,
        create: impl FnOnce() -> Result<T, String>,
    ) -> Result<&mut T, String>
    where
        T: Session + 'static,
    {
        check_kind(kind)?;
        if let Some(index) = self.entries.iter().position(|entry| entry.kind == kind) {
            return downcast(&mut self.entries[index], kind);
        }
        let session =
            create().map_err(|message| format!("could not create session `{kind}`: {message}"))?;
        self.entries.push(SessionEntry {
            kind,
            session: Box::new(session),
        });
        let entry = self
            .entries
            .last_mut()
            .ok_or_else(|| format!("could not store session `{kind}`"))?;
        downcast(entry, kind)
    }

    /// Returns the session of `kind` without creating it.
    ///
    /// A missing session is `Ok(None)`: an observation may run before the step
    /// that opens the resource. A kind that holds another type is an error.
    pub fn get<T>(&mut self, kind: &'static str) -> Result<Option<&mut T>, String>
    where
        T: Session + 'static,
    {
        check_kind(kind)?;
        match self.entries.iter().position(|entry| entry.kind == kind) {
            Some(index) => downcast(&mut self.entries[index], kind).map(Some),
            None => Ok(None),
        }
    }

    /// Closes every open session in reverse creation order.
    ///
    /// Every session is attempted, even when an earlier close fails, and each
    /// one is removed before its [`Session::close`] runs, so a failing or
    /// panicking close cannot make it close twice. The returned messages name
    /// the session kind.
    pub fn close_all(&mut self) -> Vec<String> {
        let mut errors = Vec::new();
        while let Some(mut entry) = self.entries.pop() {
            if let Err(message) = entry.session.close() {
                errors.push(format!(
                    "could not close session `{}`: {message}",
                    entry.kind
                ));
            }
        }
        errors
    }
}

impl Drop for Sessions {
    fn drop(&mut self) {
        // The normal path calls `close_all` and reports what it finds. This is
        // the fallback for a panic or an early return, where there is no report
        // left to write; it must not leave a resource open.
        let _ = self.close_all();
    }
}

/// Borrows one entry as its concrete type.
fn downcast<'a, T>(entry: &'a mut SessionEntry, kind: &'static str) -> Result<&'a mut T, String>
where
    T: Session + 'static,
{
    // The trait object is named explicitly: a method call on the box would
    // reach the blanket `AsAny` impl of `Box<dyn Session>` itself and return
    // the box instead of the session inside it.
    let session: &mut dyn Session = &mut *entry.session;
    session.as_any_mut().downcast_mut::<T>().ok_or_else(|| {
        format!(
            "session `{kind}` is already open with a different type; one session kind holds one concrete type"
        )
    })
}

/// Checks a session kind against the step name rule.
fn check_kind(kind: &str) -> Result<(), String> {
    if kind.is_empty() {
        return Err(
            "session kind is empty; use lowercase letters, digits, `.`, `_` and `-`".to_string(),
        );
    }
    if let Some(character) = kind
        .chars()
        .find(|character| !is_allowed_name_character(*character))
    {
        return Err(format!(
            "session kind `{kind}` contains `{character}`; use only lowercase letters, digits, `.`, `_` and `-`"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    #[derive(Default)]
    struct Log(Arc<Mutex<Vec<String>>>);

    impl Log {
        fn entries(&self) -> Vec<String> {
            self.0.lock().expect("the test log is not poisoned").clone()
        }
    }

    #[derive(Debug)]
    struct Tracked {
        id: usize,
        log: Arc<Mutex<Vec<String>>>,
    }

    impl Session for Tracked {
        fn close(&mut self) -> Result<(), String> {
            self.log
                .lock()
                .expect("the test log is not poisoned")
                .push(format!("close {}", self.id));
            Ok(())
        }
    }

    #[derive(Debug)]
    struct Other;

    impl Session for Other {
        fn close(&mut self) -> Result<(), String> {
            Ok(())
        }
    }

    #[derive(Debug)]
    struct Failing {
        message: String,
    }

    impl Session for Failing {
        fn close(&mut self) -> Result<(), String> {
            Err(self.message.clone())
        }
    }

    fn tracked(log: &Log, id: usize) -> Tracked {
        Tracked {
            id,
            log: Arc::clone(&log.0),
        }
    }

    #[test]
    fn one_kind_returns_one_instance_and_creates_it_once() {
        let mut sessions = Sessions::new();
        let log = Log::default();
        let mut created = 0;

        let first = sessions
            .get_or_create("test.session", || {
                created += 1;
                Ok(tracked(&log, 7))
            })
            .expect("the session must be created");
        assert_eq!(first.id, 7);

        let second = sessions
            .get_or_create("test.session", || {
                created += 1;
                Ok(tracked(&log, 8))
            })
            .expect("the existing session must be returned");
        assert_eq!(second.id, 7, "the second call must not replace the session");
        assert_eq!(created, 1, "the factory must run only on the first call");

        let found = sessions
            .get::<Tracked>("test.session")
            .expect("the lookup must succeed")
            .expect("the session must exist");
        assert_eq!(found.id, 7);

        assert!(
            sessions
                .get::<Tracked>("test.other")
                .expect("a missing kind is not a type error")
                .is_none()
        );
    }

    #[test]
    fn a_kind_that_is_already_open_with_another_type_is_an_error() {
        let mut sessions = Sessions::new();
        let log = Log::default();
        sessions
            .get_or_create("test.session", || Ok(tracked(&log, 1)))
            .expect("the session must be created");

        let error = sessions
            .get_or_create::<Other>("test.session", || Ok(Other))
            .expect_err("another type must be refused");
        assert!(error.contains("test.session"), "got: {error}");
        assert!(error.contains("different type"), "got: {error}");

        let error = sessions
            .get::<Other>("test.session")
            .expect_err("the lookup must report the type mismatch");
        assert!(error.contains("test.session"), "got: {error}");
    }

    #[test]
    fn the_factory_error_is_reported_with_the_session_kind() {
        let mut sessions = Sessions::new();

        let error = sessions
            .get_or_create::<Tracked>("test.session", || Err("no browser".to_string()))
            .expect_err("the factory failure must be reported");

        assert!(error.contains("test.session"), "got: {error}");
        assert!(error.contains("no browser"), "got: {error}");
        assert!(
            sessions
                .get::<Tracked>("test.session")
                .expect("the lookup must succeed")
                .is_none(),
            "a failed creation must not leave a slot behind"
        );
    }

    #[test]
    fn a_session_kind_follows_the_step_name_rule() {
        let mut sessions = Sessions::new();
        for kind in ["", "Test.session", "test/session", "test session"] {
            let error = sessions
                .get_or_create::<Other>(kind, || Ok(Other))
                .expect_err("the kind must be refused");
            assert!(
                error.contains("lowercase letters"),
                "the message must state the rule for `{kind}`, got: {error}"
            );
            let error = sessions
                .get::<Other>(kind)
                .expect_err("the lookup must refuse the kind");
            assert!(
                error.contains("lowercase letters"),
                "the message must state the rule for `{kind}`, got: {error}"
            );
        }
    }

    #[test]
    fn every_session_is_closed_once_in_reverse_creation_order() {
        let mut sessions = Sessions::new();
        let log = Log::default();
        for (id, kind) in ["test.session0", "test.session1", "test.session2"]
            .into_iter()
            .enumerate()
        {
            sessions
                .get_or_create(kind, || Ok(tracked(&log, id)))
                .expect("the session must be created");
        }

        let errors = sessions.close_all();
        assert!(errors.is_empty(), "got: {errors:?}");
        assert_eq!(log.entries(), vec!["close 2", "close 1", "close 0"]);

        let errors = sessions.close_all();
        assert!(errors.is_empty(), "a closed session must not close again");
        assert_eq!(
            log.entries().len(),
            3,
            "every session must close exactly once"
        );
    }

    #[test]
    fn dropping_sessions_closes_what_was_not_closed_yet() {
        let log = Log::default();
        {
            let mut sessions = Sessions::new();
            sessions
                .get_or_create("test.session", || Ok(tracked(&log, 3)))
                .expect("the session must be created");
        }

        assert_eq!(log.entries(), vec!["close 3"]);
    }

    #[test]
    fn a_close_failure_names_the_session_kind_and_keeps_the_message() {
        let mut sessions = Sessions::new();
        sessions
            .get_or_create("test.broken", || {
                Ok(Failing {
                    message: "the connection was already gone".to_string(),
                })
            })
            .expect("the session must be created");

        let errors = sessions.close_all();

        assert_eq!(errors.len(), 1, "got: {errors:?}");
        assert!(errors[0].contains("test.broken"), "got: {errors:?}");
        assert!(
            errors[0].contains("the connection was already gone"),
            "got: {errors:?}"
        );
    }
}
