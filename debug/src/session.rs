//! Shared state for a browser and an agent debugging the same program.
//!
//! Every accepted browser compile and agent edit advances one optimistic
//! revision. A request that was in flight while either collaborator changed the
//! source therefore cannot put stale text back into the shared session.

use crate::wire::{CompileRequest, SessionView};

#[derive(Debug, Clone)]
pub struct Current {
    pub revision: u64,
    pub request: CompileRequest,
    pub view: SessionView,
    client_id: String,
    agent_written: bool,
}

#[derive(Debug, Default, Clone)]
pub struct Store {
    current: Option<Current>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditError {
    NoSession,
    Conflict { current: u64 },
    WrongDocument,
    MissingFile,
}

impl Store {
    pub fn current(&self) -> Option<Current> {
        self.current.clone()
    }

    /// Observe a browser compile. Once `known_revision` is stale, the compile
    /// may still be returned to that browser, but it must not replace shared
    /// source. The response tells the browser the current revision so it can
    /// pull the winning state and continue from there.
    pub fn observe(
        &mut self,
        request: CompileRequest,
        known_revision: u64,
        client_id: String,
        view: Option<SessionView>,
    ) -> bool {
        match &mut self.current {
            // Two compiles from one browser may overlap when dependency work is
            // slow. Its later local revision is allowed to supersede its own
            // earlier compile, but never an agent edit or another browser.
            Some(current)
                if current.revision != known_revision
                    && (current.agent_written
                        || current.client_id != client_id
                        || request.revision <= current.request.revision) =>
            {
                false
            }
            Some(current) => {
                current.revision += 1;
                current.request = request;
                // A compile without a view is a client that tracks none; the
                // collaborators watching this session keep the panels they
                // have rather than being reset to defaults.
                if let Some(view) = view {
                    current.view = view;
                }
                current.client_id = client_id;
                current.agent_written = false;
                true
            }
            None if known_revision == 0 => {
                self.current = Some(Current {
                    revision: 1,
                    request,
                    view: view.unwrap_or_default(),
                    client_id,
                    agent_written: false,
                });
                true
            }
            None => false,
        }
    }

    pub fn update_view(&mut self, known_revision: u64, view: SessionView) -> Result<(), EditError> {
        let current = self.current.as_mut().ok_or(EditError::NoSession)?;
        if current.revision != known_revision {
            return Err(EditError::Conflict {
                current: current.revision,
            });
        }
        current.view = view;
        Ok(())
    }

    pub fn edit(
        &mut self,
        base_revision: u64,
        document: &str,
        path: &str,
        source: String,
    ) -> Result<Current, EditError> {
        let current = self.current.as_mut().ok_or(EditError::NoSession)?;
        if current.revision != base_revision {
            return Err(EditError::Conflict {
                current: current.revision,
            });
        }
        if current.request.document != document {
            return Err(EditError::WrongDocument);
        }
        let file = current
            .request
            .files
            .iter_mut()
            .find(|file| file.path == path)
            .ok_or(EditError::MissingFile)?;
        file.source = source;
        current.revision += 1;
        current.request.revision = current.request.revision.saturating_add(1);
        current.agent_written = true;
        Ok(current.clone())
    }
}
