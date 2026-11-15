//! plano-core : types du domaine. Aucune entrée/sortie, aucun accès disque.

use std::{fmt, str::FromStr};
use uuid::Uuid;

/// Horodatage : secondes Unix, UTC.
pub type Ts = i64;

/// Identifiant unique (UUID v7, triable par date de création).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Id(Uuid);

impl Id {
    pub fn new() -> Self {
        Id(Uuid::now_v7())
    }
}

impl Default for Id {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for Id {
    type Err = uuid::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Uuid::parse_str(s).map(Id)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemStatus {
    APlanifier,
    EnCours,
    Termine,
    AutoValide,
    Manque,
    Annule,
}

impl ItemStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            ItemStatus::APlanifier => "a_planifier",
            ItemStatus::EnCours => "en_cours",
            ItemStatus::Termine => "termine",
            ItemStatus::AutoValide => "auto_valide",
            ItemStatus::Manque => "manque",
            ItemStatus::Annule => "annule",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "a_planifier" => ItemStatus::APlanifier,
            "en_cours" => ItemStatus::EnCours,
            "termine" => ItemStatus::Termine,
            "auto_valide" => ItemStatus::AutoValide,
            "manque" => ItemStatus::Manque,
            "annule" => ItemStatus::Annule,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub id: Id,
    pub plan_id: Id,
    pub title: String,
    pub status: ItemStatus,
    pub due_at: Option<Ts>,
}

// ---------- Suites chronologiques ----------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChainState {
    NonDemarree,
    EnCours,
    Interrompue,
    Terminee,
    Abandonnee,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepState {
    EnAttente,
    Active,
    Terminee,
    AutoValidee,
    Manquee,
    Ignoree,
}

/// Échéance d'une étape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Deadline {
    /// Instant fixe.
    Absolute(Ts),
    /// N secondes après l'activation de l'étape (donc après la fin de la précédente).
    AfterPrevious(i64),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub title: String,
    pub deadline: Deadline,
    /// Échéance résolue, connue une fois l'étape activée.
    pub due_at: Option<Ts>,
    pub state: StepState,
}

impl Step {
    pub fn new(title: &str, deadline: Deadline) -> Self {
        Step {
            title: title.to_string(),
            deadline,
            due_at: None,
            state: StepState::EnAttente,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chain {
    pub id: Id,
    pub title: String,
    pub state: ChainState,
    /// Si vrai, une étape manquée est validée automatiquement (statut AutoValidee).
    pub auto_validate: bool,
    pub steps: Vec<Step>,
}

impl Chain {
    pub fn new(title: &str, steps: Vec<Step>, auto_validate: bool) -> Self {
        Chain {
            id: Id::new(),
            title: title.to_string(),
            state: ChainState::NonDemarree,
            auto_validate,
            steps,
        }
    }
}
