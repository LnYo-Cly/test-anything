//! The event bus: progress notifications emitted while cases run.

use std::time::Duration;

use crate::core::report::{CaseStatus, StepStatus, Summary};

/// A progress event.
#[derive(Debug, Clone)]
pub enum Event {
    /// A run started.
    RunStarted {
        /// Number of cases that will run.
        cases: usize,
    },
    /// A case started.
    CaseStarted {
        /// Zero-based position of the case in the run.
        case: usize,
        /// Case name.
        name: String,
    },
    /// A step started.
    StepStarted {
        /// Zero-based position of the case in the run.
        case: usize,
        /// Zero-based position of the step in the case.
        step: usize,
        /// Registered step name.
        name: String,
    },
    /// A step finished.
    StepFinished {
        /// Zero-based position of the case in the run.
        case: usize,
        /// Zero-based position of the step in the case.
        step: usize,
        /// Registered step name.
        name: String,
        /// Step state.
        status: StepStatus,
        /// How long the step took.
        duration: Duration,
    },
    /// A case finished.
    CaseFinished {
        /// Zero-based position of the case in the run.
        case: usize,
        /// Case name.
        name: String,
        /// Case state.
        status: CaseStatus,
        /// How long the case took.
        duration: Duration,
    },
    /// A run finished.
    RunFinished {
        /// Counts for the run.
        summary: Summary,
        /// How long the run took.
        duration: Duration,
    },
}

type Listener = Box<dyn Fn(&Event) + Send + Sync>;

/// Delivers [`Event`]s to every subscribed listener, in subscription order.
#[derive(Default)]
pub struct EventBus {
    listeners: Vec<Listener>,
}

impl EventBus {
    /// Creates a bus without listeners.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a listener.
    ///
    /// Listeners are called synchronously, before the next event is emitted,
    /// so the order they observe is the order the runner emitted.
    pub fn subscribe<F>(&mut self, listener: F)
    where
        F: Fn(&Event) + Send + Sync + 'static,
    {
        self.listeners.push(Box::new(listener));
    }

    /// Sends an event to every listener.
    pub(crate) fn emit(&self, event: Event) {
        for listener in &self.listeners {
            listener(&event);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    #[test]
    fn listeners_receive_events_in_the_order_they_were_emitted() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let recorder = Arc::clone(&seen);
        let mut bus = EventBus::new();
        bus.subscribe(move |event| {
            let name = match event {
                Event::RunStarted { .. } => "run started",
                Event::CaseStarted { .. } => "case started",
                Event::StepStarted { .. } => "step started",
                Event::StepFinished { .. } => "step finished",
                Event::CaseFinished { .. } => "case finished",
                Event::RunFinished { .. } => "run finished",
            };
            recorder
                .lock()
                .expect("the recording mutex is poisoned")
                .push(name);
        });

        bus.emit(Event::RunStarted { cases: 1 });
        bus.emit(Event::CaseStarted {
            case: 0,
            name: "case".to_string(),
        });
        bus.emit(Event::RunFinished {
            summary: Summary::default(),
            duration: Duration::ZERO,
        });

        assert_eq!(
            *seen.lock().expect("the recording mutex is poisoned"),
            vec!["run started", "case started", "run finished"]
        );
    }
}
