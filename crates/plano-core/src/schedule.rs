//! Types de planification : déclencheurs, durées résolues, rappels.
//! Aucune entrée/sortie : ces types sont purs.

use crate::{Id, Ts};

/// Bord d'un élément auquel un autre peut s'ancrer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edge {
    Start,
    End,
}

/// Intervalle résolu d'un élément (secondes Unix, UTC).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: Ts,
    pub end: Ts,
}

/// Ce qui place un élément dans le temps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Trigger {
    /// Instant fixe.
    Absolute(Ts),
    /// Relatif au début ou à la fin d'un autre élément (décalage négatif = avant).
    RelativeTo {
        anchor: Id,
        edge: Edge,
        offset_s: i64,
    },
    /// Non planifié : l'utilisateur le déclenche à la main.
    Manual,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduleItem {
    pub id: Id,
    pub trigger: Trigger,
    pub duration_s: i64,
}

/// Jours de la semaine, en masque de bits (lundi = bit 0).
pub mod weekday {
    pub const MON: u8 = 1;
    pub const TUE: u8 = 2;
    pub const WED: u8 = 4;
    pub const THU: u8 = 8;
    pub const FRI: u8 = 16;
    pub const SAT: u8 = 32;
    pub const SUN: u8 = 64;
    pub const WORKDAYS: u8 = MON | TUE | WED | THU | FRI;
    pub const ALL: u8 = WORKDAYS | SAT | SUN;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReminderRule {
    /// À un instant précis.
    At(Ts),
    /// N secondes avant le début d'un élément.
    BeforeItem { item: Id, lead_s: i64 },
    /// Chaque jour (ou certains jours) à une heure locale.
    Daily { hour: u8, minute: u8, weekdays: u8 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reminder {
    pub id: Id,
    pub rule: ReminderRule,
    pub last_fired_at: Option<Ts>,
    pub snoozed_until: Option<Ts>,
}

impl Reminder {
    pub fn new(rule: ReminderRule) -> Self {
        Reminder {
            id: Id::new(),
            rule,
            last_fired_at: None,
            snoozed_until: None,
        }
    }
}
