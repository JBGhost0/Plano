//! Déclencheurs relatifs, rappels, réveil du planificateur et rattrapage.
//! Logique pure : le fuseau horaire est injecté (trait `Tz`).

use plano_core::schedule::{Edge, Reminder, ReminderRule, ScheduleItem, Span, Trigger};
use plano_core::{Id, Ts};
use std::collections::HashMap;
use std::fmt;

const DAY: i64 = 86_400;

/// Décalage entre l'heure locale et UTC, en secondes, à un instant donné.
/// L'application fournit l'implémentation qui gère le changement d'heure.
pub trait Tz {
    fn offset_at(&self, ts: Ts) -> i64;
}

/// Décalage constant (UTC = `FixedOffset(0)`).
pub struct FixedOffset(pub i64);

impl Tz for FixedOffset {
    fn offset_at(&self, _ts: Ts) -> i64 {
        self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScheduleError {
    Cycle(Id),
    MissingAnchor { item: Id, anchor: Id },
}

impl fmt::Display for ScheduleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ScheduleError::Cycle(id) => write!(f, "dépendance circulaire sur l'élément {id}"),
            ScheduleError::MissingAnchor { item, anchor } => {
                write!(f, "l'élément {item} dépend de {anchor}, introuvable")
            }
        }
    }
}

impl std::error::Error for ScheduleError {}

// ---------- Résolution des déclencheurs ----------

/// Calcule l'intervalle de chaque élément planifié. Les éléments `Manual`
/// (ou ancrés sur un élément `Manual`) n'apparaissent pas dans le résultat.
pub fn resolve_all(items: &[ScheduleItem]) -> Result<HashMap<Id, Span>, ScheduleError> {
    let by_id: HashMap<Id, &ScheduleItem> = items.iter().map(|i| (i.id, i)).collect();
    let mut done: HashMap<Id, Option<Span>> = HashMap::new();
    let mut visiting: Vec<Id> = Vec::new();
    for it in items {
        resolve_one(it.id, &by_id, &mut done, &mut visiting)?;
    }
    Ok(done
        .into_iter()
        .filter_map(|(id, span)| span.map(|s| (id, s)))
        .collect())
}

fn resolve_one(
    id: Id,
    by_id: &HashMap<Id, &ScheduleItem>,
    done: &mut HashMap<Id, Option<Span>>,
    visiting: &mut Vec<Id>,
) -> Result<Option<Span>, ScheduleError> {
    if let Some(span) = done.get(&id) {
        return Ok(*span);
    }
    if visiting.contains(&id) {
        return Err(ScheduleError::Cycle(id));
    }
    let item = by_id[&id];
    visiting.push(id);
    let span = match item.trigger {
        Trigger::Manual => None,
        Trigger::Absolute(t) => Some(Span {
            start: t,
            end: t + item.duration_s,
        }),
        Trigger::RelativeTo {
            anchor,
            edge,
            offset_s,
        } => {
            if !by_id.contains_key(&anchor) {
                return Err(ScheduleError::MissingAnchor { item: id, anchor });
            }
            resolve_one(anchor, by_id, done, visiting)?.map(|a| {
                let base = match edge {
                    Edge::Start => a.start,
                    Edge::End => a.end,
                };
                let start = base + offset_s;
                Span {
                    start,
                    end: start + item.duration_s,
                }
            })
        }
    };
    visiting.pop();
    done.insert(id, span);
    Ok(span)
}

// ---------- Rappels ----------

/// Prochain déclenchement strictement postérieur à `after`.
pub fn next_fire(
    rule: &ReminderRule,
    after: Ts,
    spans: &HashMap<Id, Span>,
    tz: &dyn Tz,
) -> Option<Ts> {
    match *rule {
        ReminderRule::At(t) => (t > after).then_some(t),
        ReminderRule::BeforeItem { item, lead_s } => {
            let t = spans.get(&item)?.start - lead_s;
            (t > after).then_some(t)
        }
        ReminderRule::Daily {
            hour,
            minute,
            weekdays,
        } => next_daily(hour, minute, weekdays, after, tz),
    }
}

fn next_daily(hour: u8, minute: u8, weekdays: u8, after: Ts, tz: &dyn Tz) -> Option<Ts> {
    if hour > 23 || minute > 59 || weekdays & 0x7f == 0 {
        return None;
    }
    let sec_of_day = i64::from(hour) * 3600 + i64::from(minute) * 60;
    let first_day = (after + tz.offset_at(after)).div_euclid(DAY);
    for d in 0..=8 {
        let day = first_day + d;
        // Le 1er janvier 1970 était un jeudi ; lundi = 0.
        let wd = (day + 3).rem_euclid(7) as u32;
        if weekdays & (1u8 << wd) == 0 {
            continue;
        }
        let local = day * DAY + sec_of_day;
        let guess = local - tz.offset_at(after);
        let utc = local - tz.offset_at(guess);
        if utc > after {
            return Some(utc);
        }
    }
    None
}

/// Prochain réveil du planificateur : l'instant et le rappel concerné.
/// Un rappel reporté (`snoozed_until`) passe avant son déclenchement normal.
pub fn next_wake(
    reminders: &[Reminder],
    now: Ts,
    spans: &HashMap<Id, Span>,
    tz: &dyn Tz,
) -> Option<(Ts, Id)> {
    reminders
        .iter()
        .filter_map(|r| {
            if let Some(s) = r.snoozed_until {
                if s > now {
                    return Some((s, r.id));
                }
            }
            next_fire(&r.rule, now, spans, tz).map(|t| (t, r.id))
        })
        .min_by_key(|(t, _)| *t)
}

/// Rappel manqué pendant que l'application était fermée.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Missed {
    pub reminder: Id,
    /// Dernier déclenchement manqué.
    pub latest_at: Ts,
    /// Nombre de déclenchements manqués (un rappel quotidien peut en cumuler plusieurs).
    pub count: u32,
}

/// À appeler au lancement : liste (sans rafale) ce qui s'est déclenché entre la
/// dernière exécution et maintenant.
pub fn catch_up(
    reminders: &[Reminder],
    last_run: Ts,
    now: Ts,
    spans: &HashMap<Id, Span>,
    tz: &dyn Tz,
) -> Vec<Missed> {
    let mut out = Vec::new();
    for r in reminders {
        let mut cursor = r.last_fired_at.map_or(last_run, |f| f.max(last_run));
        let mut count = 0u32;
        let mut latest = 0;
        while let Some(t) = next_fire(&r.rule, cursor, spans, tz) {
            if t > now {
                break;
            }
            count += 1;
            latest = t;
            cursor = t;
            if count >= 1000 {
                break;
            }
        }
        if count > 0 {
            out.push(Missed {
                reminder: r.id,
                latest_at: latest,
                count,
            });
        }
    }
    out
}

/// Enregistre qu'un rappel a été présenté à l'utilisateur.
pub fn mark_fired(reminder: &mut Reminder, at: Ts) {
    reminder.last_fired_at = Some(at);
    reminder.snoozed_until = None;
}

/// Reporte un rappel (« dans 10 min »).
pub fn snooze(reminder: &mut Reminder, now: Ts, seconds: i64) {
    reminder.snoozed_until = Some(now + seconds);
}

#[cfg(test)]
mod tests {
    use super::*;
    use plano_core::schedule::weekday;

    const UTC: FixedOffset = FixedOffset(0);

    fn item(trigger: Trigger, duration_s: i64) -> ScheduleItem {
        ScheduleItem {
            id: Id::new(),
            trigger,
            duration_s,
        }
    }

    fn daily(hour: u8, minute: u8, weekdays: u8) -> Reminder {
        Reminder::new(ReminderRule::Daily {
            hour,
            minute,
            weekdays,
        })
    }

    #[test]
    fn depart_salle_suit_la_seance() {
        let seance = item(Trigger::Absolute(64_800), 5_400);
        let depart = item(
            Trigger::RelativeTo {
                anchor: seance.id,
                edge: Edge::Start,
                offset_s: -(1_800 + 600),
            },
            0,
        );
        let spans = resolve_all(&[seance.clone(), depart.clone()]).unwrap();
        assert_eq!(spans[&depart.id].start, 62_400);

        // La séance est décalée : le départ suit.
        let mut seance2 = seance.clone();
        seance2.trigger = Trigger::Absolute(66_600);
        let spans = resolve_all(&[seance2, depart.clone()]).unwrap();
        assert_eq!(spans[&depart.id].start, 64_200);
    }

    #[test]
    fn chaine_de_relatifs() {
        let a = item(Trigger::Absolute(1_000), 100);
        let b = item(
            Trigger::RelativeTo {
                anchor: a.id,
                edge: Edge::End,
                offset_s: 60,
            },
            50,
        );
        let c = item(
            Trigger::RelativeTo {
                anchor: b.id,
                edge: Edge::Start,
                offset_s: 10,
            },
            0,
        );
        // L'ordre dans la liste n'a pas d'importance.
        let spans = resolve_all(&[c.clone(), b.clone(), a]).unwrap();
        assert_eq!(
            spans[&b.id],
            Span {
                start: 1_160,
                end: 1_210
            }
        );
        assert_eq!(spans[&c.id].start, 1_170);
    }

    #[test]
    fn dependance_circulaire_refusee() {
        let (a, b) = (Id::new(), Id::new());
        let ia = ScheduleItem {
            id: a,
            trigger: Trigger::RelativeTo {
                anchor: b,
                edge: Edge::Start,
                offset_s: 0,
            },
            duration_s: 0,
        };
        let ib = ScheduleItem {
            id: b,
            trigger: Trigger::RelativeTo {
                anchor: a,
                edge: Edge::Start,
                offset_s: 0,
            },
            duration_s: 0,
        };
        assert!(matches!(
            resolve_all(&[ia, ib]),
            Err(ScheduleError::Cycle(_))
        ));
    }

    #[test]
    fn ancre_introuvable() {
        let orphelin = item(
            Trigger::RelativeTo {
                anchor: Id::new(),
                edge: Edge::Start,
                offset_s: 0,
            },
            0,
        );
        assert!(matches!(
            resolve_all(&[orphelin]),
            Err(ScheduleError::MissingAnchor { .. })
        ));
    }

    #[test]
    fn manuel_reste_non_planifie() {
        let m = item(Trigger::Manual, 0);
        let spans = resolve_all(&[m.clone()]).unwrap();
        assert!(!spans.contains_key(&m.id));
    }

    #[test]
    fn quotidien_jours_ouvres() {
        // Vendredi 1970-01-09 à 09:00 UTC -> prochain : lundi 1970-01-12 à 08:00.
        let after = 8 * 86_400 + 9 * 3_600;
        let rule = ReminderRule::Daily {
            hour: 8,
            minute: 0,
            weekdays: weekday::WORKDAYS,
        };
        let t = next_fire(&rule, after, &HashMap::new(), &UTC);
        assert_eq!(t, Some(11 * 86_400 + 8 * 3_600));
    }

    #[test]
    fn quotidien_heure_locale() {
        // UTC+1 : 08:00 locale = 07:00 UTC.
        let rule = ReminderRule::Daily {
            hour: 8,
            minute: 0,
            weekdays: weekday::ALL,
        };
        let t = next_fire(&rule, 0, &HashMap::new(), &FixedOffset(3_600));
        assert_eq!(t, Some(7 * 3_600));
    }

    #[test]
    fn quotidien_invalide() {
        let rule = ReminderRule::Daily {
            hour: 25,
            minute: 0,
            weekdays: weekday::ALL,
        };
        assert_eq!(next_fire(&rule, 0, &HashMap::new(), &UTC), None);
    }

    #[test]
    fn avant_un_element() {
        let seance = item(Trigger::Absolute(64_800), 5_400);
        let spans = resolve_all(&[seance.clone()]).unwrap();
        let rule = ReminderRule::BeforeItem {
            item: seance.id,
            lead_s: 900,
        };
        assert_eq!(next_fire(&rule, 60_000, &spans, &UTC), Some(63_900));
        assert_eq!(next_fire(&rule, 64_000, &spans, &UTC), None);
    }

    #[test]
    fn reveil_et_report() {
        let now = 9 * 3_600;
        let mut r1 = daily(8, 0, weekday::ALL);
        let r2 = Reminder::new(ReminderRule::At(now + 100));
        let spans = HashMap::new();
        let rs = [r1.clone(), r2.clone()];
        assert_eq!(next_wake(&rs, now, &spans, &UTC), Some((now + 100, r2.id)));

        snooze(&mut r1, now, 10);
        let rs = [r1.clone(), r2];
        assert_eq!(next_wake(&rs, now, &spans, &UTC), Some((now + 10, r1.id)));
    }

    #[test]
    fn rattrapage_sans_rafale() {
        // Fermé du jour 0 à 00:00 jusqu'au jour 3 à 12:00 : 4 passages à 08:00 manqués.
        let mut r = daily(8, 0, weekday::ALL);
        let now = 3 * 86_400 + 12 * 3_600;
        let spans = HashMap::new();

        let missed = catch_up(&[r.clone()], 0, now, &spans, &UTC);
        assert_eq!(
            missed,
            vec![Missed {
                reminder: r.id,
                latest_at: 3 * 86_400 + 8 * 3_600,
                count: 4,
            }]
        );

        // Une fois présenté, il n'est plus rattrapé.
        mark_fired(&mut r, missed[0].latest_at);
        assert!(catch_up(&[r], 0, now, &spans, &UTC).is_empty());
    }

    #[test]
    fn rappel_futur_non_rattrape() {
        let r = Reminder::new(ReminderRule::At(10_000));
        assert!(catch_up(&[r], 0, 5_000, &HashMap::new(), &UTC).is_empty());
    }
}
