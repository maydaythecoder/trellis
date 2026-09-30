//! The label schema: six dimensions of one value each, the flags, and the
//! triage route (standard §3 and §6). Each value knows its label text, so
//! the spelling lives in one place.

/// `area:` — which component (§3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Area {
    Context,
    Model,
    Checks,
    Runtime,
    Skills,
    Agents,
    Protocol,
    Tui,
    Graph,
    Cli,
    Build,
    Docs,
}

impl Area {
    pub fn label(self) -> &'static str {
        match self {
            Area::Context => "area:context",
            Area::Model => "area:model",
            Area::Checks => "area:checks",
            Area::Runtime => "area:runtime",
            Area::Skills => "area:skills",
            Area::Agents => "area:agents",
            Area::Protocol => "area:protocol",
            Area::Tui => "area:tui",
            Area::Graph => "area:graph",
            Area::Cli => "area:cli",
            Area::Build => "area:build",
            Area::Docs => "area:docs",
        }
    }
}

/// `kind:` — what sort of work (§3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Feature,
    Bug,
    Refactor,
    Chore,
    Test,
    Docs,
    Spike,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Feature => "kind:feature",
            Kind::Bug => "kind:bug",
            Kind::Refactor => "kind:refactor",
            Kind::Chore => "kind:chore",
            Kind::Test => "kind:test",
            Kind::Docs => "kind:docs",
            Kind::Spike => "kind:spike",
        }
    }
}

/// `risk:` — blast radius if it goes wrong (§3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Risk {
    Low,
    Medium,
    High,
}

impl Risk {
    pub fn label(self) -> &'static str {
        match self {
            Risk::Low => "risk:low",
            Risk::Medium => "risk:medium",
            Risk::High => "risk:high",
        }
    }
}

/// `surface:` — who sees a change here (§3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Surface {
    Internal,
    Cli,
    Protocol,
    Public,
}

impl Surface {
    pub fn label(self) -> &'static str {
        match self {
            Surface::Internal => "surface:internal",
            Surface::Cli => "surface:cli",
            Surface::Protocol => "surface:protocol",
            Surface::Public => "surface:public",
        }
    }
}

/// `effort:` — size (§3). `L` means "split it" and is never ready.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effort {
    S,
    M,
    L,
}

impl Effort {
    pub fn label(self) -> &'static str {
        match self {
            Effort::S => "effort:s",
            Effort::M => "effort:m",
            Effort::L => "effort:l",
        }
    }
}

/// `origin:` — where it came from (§3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Manual,
    Plan,
    Code,
    Check,
    Agent,
    Qa,
    Revived,
}

impl Origin {
    const ALL: [Origin; 7] = [
        Origin::Manual,
        Origin::Plan,
        Origin::Code,
        Origin::Check,
        Origin::Agent,
        Origin::Qa,
        Origin::Revived,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Origin::Manual => "origin:manual",
            Origin::Plan => "origin:plan",
            Origin::Code => "origin:code",
            Origin::Check => "origin:check",
            Origin::Agent => "origin:agent",
            Origin::Qa => "origin:qa",
            Origin::Revived => "origin:revived",
        }
    }

    pub(crate) fn from_label(label: &str) -> Option<Origin> {
        Origin::ALL.into_iter().find(|o| o.label() == label)
    }
}

/// The flags: boolean, absent by default (§3). Labels carry the `!`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Flag {
    AgentOk,
    NeedsAdr,
    Breaking,
    Security,
    BlockedExt,
}

impl Flag {
    pub fn label(self) -> &'static str {
        match self {
            Flag::AgentOk => "!agent-ok",
            Flag::NeedsAdr => "!needs-adr",
            Flag::Breaking => "!breaking",
            Flag::Security => "!security",
            Flag::BlockedExt => "!blocked-ext",
        }
    }
}

/// `triage:` — where the ticket goes next (§6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    Auto,
    NeedsHuman,
    Blocked,
}

impl Route {
    pub fn label(self) -> &'static str {
        match self {
            Route::Auto => "triage:auto",
            Route::NeedsHuman => "triage:needs-human",
            Route::Blocked => "triage:blocked",
        }
    }
}
