// The Herdr Briardr talks to: Herdr TUI's server or herdr-core (under the Herdr app), chosen by
// `herdrBackend` and switchable at runtime. The rest of the server holds this handle; its event
// channel outlives a switch, so listeners stay subscribed.
use std::sync::{Arc, RwLock};

use tokio::sync::broadcast;

use super::core::state::{core_status, CoreState, CoreTarget};
use super::model::Model;
use super::rpc::{herdr_status, HerdrError, HerdrStatus, HerdrTarget};
use super::state::{HerdrEvent, HerdrHealth, HerdrReader, HerdrState};
use crate::config::BriardrConfig;
use crate::diag::Diagnostics;

const EVENT_CAPACITY: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Tui(HerdrTarget),
    Core(CoreTarget),
}

impl Target {
    pub fn of(cfg: &BriardrConfig) -> Self {
        if cfg.herdr_backend == "core" {
            Target::Core(CoreTarget::of(cfg))
        } else {
            Target::Tui(HerdrTarget::of(cfg))
        }
    }

    /// A one-off check, for Settings and `briardr status`. Never starts anything.
    pub async fn status(&self) -> HerdrStatus {
        match self {
            Target::Tui(t) => herdr_status(t).await,
            Target::Core(t) => core_status(t).await,
        }
    }
}

#[derive(Clone)]
pub enum Backend {
    Tui(HerdrState),
    Core(CoreState),
}

impl Backend {
    fn new(target: Target, diag: Arc<Diagnostics>, events: broadcast::Sender<HerdrEvent>) -> Self {
        match target {
            Target::Tui(t) => Backend::Tui(HerdrState::new(t, diag, events)),
            Target::Core(t) => Backend::Core(CoreState::new(t, diag, events)),
        }
    }

    fn reader(&self) -> &dyn HerdrReader {
        match self {
            Backend::Tui(h) => h,
            Backend::Core(c) => c,
        }
    }

    async fn start(&self) {
        match self {
            Backend::Tui(h) => h.start().await,
            Backend::Core(c) => c.start().await,
        }
    }

    fn stop(&self) {
        match self {
            Backend::Tui(h) => h.stop(),
            Backend::Core(c) => c.stop(),
        }
    }
}

struct Shared {
    backend: RwLock<Backend>,
    events: broadcast::Sender<HerdrEvent>,
    diag: Arc<Diagnostics>,
}

/// Cheap to clone; clones share the backend.
#[derive(Clone)]
pub struct Herdr {
    shared: Arc<Shared>,
}

impl Herdr {
    pub fn new(target: Target, diag: Arc<Diagnostics>) -> Self {
        let (events, _) = broadcast::channel(EVENT_CAPACITY);
        let backend = Backend::new(target, diag.clone(), events.clone());
        Herdr {
            shared: Arc::new(Shared {
                backend: RwLock::new(backend),
                events,
                diag,
            }),
        }
    }

    pub fn backend(&self) -> Backend {
        self.shared
            .backend
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub async fn start(&self) {
        self.backend().start().await;
    }

    pub fn stop(&self) {
        self.backend().stop();
    }

    /// Points Briardr at another Herdr: the same kind is retargeted, another kind replaces it.
    pub async fn retarget(&self, target: Target) {
        let next = match (self.backend(), target) {
            (Backend::Tui(h), Target::Tui(t)) => return h.retarget(t).await,
            (Backend::Core(c), Target::Core(t)) => return c.retarget(t).await,
            (old, target) => {
                old.stop();
                Backend::new(target, self.shared.diag.clone(), self.shared.events.clone())
            }
        };
        *self
            .shared
            .backend
            .write()
            .unwrap_or_else(|e| e.into_inner()) = next.clone();
        // Screens drop the old Herdr's model now, not when the new one first reports.
        let _ = self
            .shared
            .events
            .send(HerdrEvent::Health(next.reader().health()));
        next.start().await;
    }

    pub async fn refresh(&self) -> Result<(), HerdrError> {
        match self.backend() {
            Backend::Tui(h) => h.refresh().await,
            Backend::Core(c) => c.refresh().await,
        }
    }
}

impl HerdrReader for Herdr {
    fn model(&self) -> Option<Model> {
        self.backend().reader().model()
    }

    fn health(&self) -> HerdrHealth {
        self.backend().reader().health()
    }

    fn events(&self) -> broadcast::Receiver<HerdrEvent> {
        self.shared.events.subscribe()
    }
}
