//! Where a live scene came from — attribution for audit / future promotion.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ProvenanceSource {
    #[default]
    Builtin,
    /// Presented via `PresentScene` IPC (shell or stdin harness).
    Ipc,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Provenance {
    pub source: ProvenanceSource,
}
