//! plano-store : persistance locale SQLite (mode WAL, migrations par user_version).

use plano_core::schedule::{Edge, Reminder, ReminderRule, ScheduleItem, Trigger};
use plano_core::{Id, Item, ItemStatus, Ts};
use rusqlite::{params, Connection, OptionalExtension};

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("erreur SQLite : {0}")]
    Sql(#[from] rusqlite::Error),
    #[error("donnée invalide : {0}")]
    Data(String),
}

const SCHEMA_V1: &str = "
BEGIN;
CREATE TABLE plan (
  id TEXT PRIMARY KEY,
  title TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  deleted_at INTEGER
);
CREATE TABLE item (
  id TEXT PRIMARY KEY,
  plan_id TEXT NOT NULL REFERENCES plan(id),
  title TEXT NOT NULL,
  status TEXT NOT NULL,
  due_at INTEGER,
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL,
  deleted_at INTEGER
);
CREATE INDEX idx_item_plan_status ON item(plan_id, status);
PRAGMA user_version = 1;
COMMIT;
";

const SCHEMA_V2: &str = "
BEGIN;
ALTER TABLE item ADD COLUMN trigger_kind TEXT;
ALTER TABLE item ADD COLUMN trigger_at INTEGER;
ALTER TABLE item ADD COLUMN trigger_anchor TEXT;
ALTER TABLE item ADD COLUMN trigger_edge TEXT;
ALTER TABLE item ADD COLUMN trigger_offset INTEGER;
ALTER TABLE item ADD COLUMN duration_s INTEGER NOT NULL DEFAULT 0;
CREATE TABLE reminder (
  id TEXT PRIMARY KEY,
  label TEXT NOT NULL,
  kind TEXT NOT NULL,
  fire_at INTEGER,
  item_id TEXT,
  lead_s INTEGER,
  hour INTEGER,
  minute INTEGER,
  weekdays INTEGER,
  last_fired_at INTEGER,
  snoozed_until INTEGER,
  created_at INTEGER NOT NULL,
  deleted_at INTEGER
);
CREATE TABLE app_setting (
  name TEXT PRIMARY KEY,
  value TEXT NOT NULL
);
PRAGMA user_version = 2;
COMMIT;
";

pub struct Store {
    conn: Connection,
}

fn parse_id(s: &str) -> Result<Id, StoreError> {
    s.parse::<Id>()
        .map_err(|e| StoreError::Data(format!("{e}")))
}

fn missing(what: &str) -> StoreError {
    StoreError::Data(format!("champ manquant : {what}"))
}

// ---------- Conversion déclencheurs <-> colonnes ----------

struct TriggerCols {
    kind: Option<&'static str>,
    at: Option<i64>,
    anchor: Option<String>,
    edge: Option<&'static str>,
    offset: Option<i64>,
}

fn trigger_cols(trigger: Trigger) -> TriggerCols {
    match trigger {
        Trigger::Absolute(t) => TriggerCols {
            kind: Some("absolute"),
            at: Some(t),
            anchor: None,
            edge: None,
            offset: None,
        },
        Trigger::RelativeTo {
            anchor,
            edge,
            offset_s,
        } => TriggerCols {
            kind: Some("relative"),
            at: None,
            anchor: Some(anchor.to_string()),
            edge: Some(match edge {
                Edge::Start => "start",
                Edge::End => "end",
            }),
            offset: Some(offset_s),
        },
        Trigger::Manual => TriggerCols {
            kind: None,
            at: None,
            anchor: None,
            edge: None,
            offset: None,
        },
    }
}

fn decode_trigger(
    kind: &str,
    at: Option<i64>,
    anchor: Option<String>,
    edge: Option<String>,
    offset: Option<i64>,
) -> Result<Trigger, StoreError> {
    match kind {
        "absolute" => Ok(Trigger::Absolute(at.ok_or_else(|| missing("trigger_at"))?)),
        "relative" => {
            let anchor = parse_id(&anchor.ok_or_else(|| missing("trigger_anchor"))?)?;
            let edge = match edge.as_deref() {
                Some("start") => Edge::Start,
                Some("end") => Edge::End,
                _ => return Err(StoreError::Data("bord d'ancrage invalide".to_string())),
            };
            Ok(Trigger::RelativeTo {
                anchor,
                edge,
                offset_s: offset.unwrap_or(0),
            })
        }
        other => Err(StoreError::Data(format!("déclencheur inconnu : {other}"))),
    }
}

// ---------- Conversion rappels <-> colonnes ----------

struct RuleCols {
    kind: &'static str,
    fire_at: Option<i64>,
    item_id: Option<String>,
    lead_s: Option<i64>,
    hour: Option<i64>,
    minute: Option<i64>,
    weekdays: Option<i64>,
}

fn rule_cols(rule: ReminderRule) -> RuleCols {
    match rule {
        ReminderRule::At(t) => RuleCols {
            kind: "at",
            fire_at: Some(t),
            item_id: None,
            lead_s: None,
            hour: None,
            minute: None,
            weekdays: None,
        },
        ReminderRule::BeforeItem { item, lead_s } => RuleCols {
            kind: "before_item",
            fire_at: None,
            item_id: Some(item.to_string()),
            lead_s: Some(lead_s),
            hour: None,
            minute: None,
            weekdays: None,
        },
        ReminderRule::Daily {
            hour,
            minute,
            weekdays,
        } => RuleCols {
            kind: "daily",
            fire_at: None,
            item_id: None,
            lead_s: None,
            hour: Some(i64::from(hour)),
            minute: Some(i64::from(minute)),
            weekdays: Some(i64::from(weekdays)),
        },
    }
}

struct RawReminder {
    id: String,
    label: String,
    kind: String,
    fire_at: Option<i64>,
    item_id: Option<String>,
    lead_s: Option<i64>,
    hour: Option<i64>,
    minute: Option<i64>,
    weekdays: Option<i64>,
    last_fired_at: Option<i64>,
    snoozed_until: Option<i64>,
}

impl Store {
    pub fn open(path: &str) -> Result<Self, StoreError> {
        let conn = Connection::open(path)?;
        conn.execute_batch("PRAGMA journal_mode = WAL; PRAGMA foreign_keys = ON;")?;
        let store = Store { conn };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&self) -> Result<(), StoreError> {
        let version: i64 = self
            .conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version < 1 {
            self.conn.execute_batch(SCHEMA_V1)?;
        }
        if version < 2 {
            self.conn.execute_batch(SCHEMA_V2)?;
        }
        Ok(())
    }

    // ---------- Plans et tâches ----------

    pub fn add_plan(&self, title: &str, now: Ts) -> Result<Id, StoreError> {
        let id = Id::new();
        self.conn.execute(
            "INSERT INTO plan (id, title, created_at, updated_at) VALUES (?1, ?2, ?3, ?3)",
            params![id.to_string(), title, now],
        )?;
        Ok(id)
    }

    pub fn list_plans(&self) -> Result<Vec<(Id, String)>, StoreError> {
        let mut st = self
            .conn
            .prepare("SELECT id, title FROM plan WHERE deleted_at IS NULL ORDER BY created_at")?;
        let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
        let mut out = Vec::new();
        for row in rows {
            let (id, title) = row?;
            out.push((parse_id(&id)?, title));
        }
        Ok(out)
    }

    pub fn add_item(&self, plan_id: Id, title: &str, now: Ts) -> Result<Id, StoreError> {
        let id = Id::new();
        self.conn.execute(
            "INSERT INTO item (id, plan_id, title, status, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
            params![
                id.to_string(),
                plan_id.to_string(),
                title,
                ItemStatus::APlanifier.as_str(),
                now
            ],
        )?;
        Ok(id)
    }

    pub fn list_items(&self, plan_id: Id) -> Result<Vec<Item>, StoreError> {
        let mut st = self.conn.prepare(
            "SELECT id, plan_id, title, status, due_at FROM item
             WHERE plan_id = ?1 AND deleted_at IS NULL ORDER BY created_at",
        )?;
        let rows = st.query_map(params![plan_id.to_string()], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<i64>>(4)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, plan, title, status, due_at) = row?;
            out.push(Item {
                id: parse_id(&id)?,
                plan_id: parse_id(&plan)?,
                title,
                status: ItemStatus::parse(&status)
                    .ok_or_else(|| StoreError::Data(format!("statut inconnu : {status}")))?,
                due_at,
            });
        }
        Ok(out)
    }

    pub fn item_title(&self, id: Id) -> Result<Option<String>, StoreError> {
        Ok(self
            .conn
            .query_row(
                "SELECT title FROM item WHERE id = ?1 AND deleted_at IS NULL",
                params![id.to_string()],
                |r| r.get(0),
            )
            .optional()?)
    }

    pub fn set_status(&self, item_id: Id, status: ItemStatus, now: Ts) -> Result<bool, StoreError> {
        let n = self.conn.execute(
            "UPDATE item SET status = ?1, updated_at = ?2 WHERE id = ?3 AND deleted_at IS NULL",
            params![status.as_str(), now, item_id.to_string()],
        )?;
        Ok(n > 0)
    }

    // ---------- Déclencheurs ----------

    /// Enregistre le déclencheur et la durée d'une tâche (`Trigger::Manual` l'efface).
    pub fn set_trigger(
        &self,
        item_id: Id,
        trigger: Trigger,
        duration_s: i64,
        now: Ts,
    ) -> Result<bool, StoreError> {
        let c = trigger_cols(trigger);
        let n = self.conn.execute(
            "UPDATE item SET trigger_kind = ?1, trigger_at = ?2, trigger_anchor = ?3,
                    trigger_edge = ?4, trigger_offset = ?5, duration_s = ?6, updated_at = ?7
             WHERE id = ?8 AND deleted_at IS NULL",
            params![
                c.kind,
                c.at,
                c.anchor,
                c.edge,
                c.offset,
                duration_s,
                now,
                item_id.to_string()
            ],
        )?;
        Ok(n > 0)
    }

    /// Toutes les tâches planifiées (tous plans confondus : les liens traversent les plans).
    pub fn list_schedule_items(&self) -> Result<Vec<ScheduleItem>, StoreError> {
        let mut st = self.conn.prepare(
            "SELECT id, trigger_kind, trigger_at, trigger_anchor, trigger_edge,
                    trigger_offset, duration_s
             FROM item WHERE deleted_at IS NULL AND trigger_kind IS NOT NULL",
        )?;
        let rows = st.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, Option<i64>>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, Option<i64>>(5)?,
                r.get::<_, i64>(6)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (id, kind, at, anchor, edge, offset, duration_s) = row?;
            out.push(ScheduleItem {
                id: parse_id(&id)?,
                trigger: decode_trigger(&kind, at, anchor, edge, offset)?,
                duration_s,
            });
        }
        Ok(out)
    }

    // ---------- Rappels ----------

    pub fn add_reminder(&self, rule: ReminderRule, label: &str, now: Ts) -> Result<Id, StoreError> {
        let reminder = Reminder::new(rule);
        let c = rule_cols(rule);
        self.conn.execute(
            "INSERT INTO reminder (id, label, kind, fire_at, item_id, lead_s, hour, minute,
                                   weekdays, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                reminder.id.to_string(),
                label,
                c.kind,
                c.fire_at,
                c.item_id,
                c.lead_s,
                c.hour,
                c.minute,
                c.weekdays,
                now
            ],
        )?;
        Ok(reminder.id)
    }

    /// Rappels avec leur libellé, du plus ancien au plus récent.
    pub fn list_reminders(&self) -> Result<Vec<(Reminder, String)>, StoreError> {
        let mut st = self.conn.prepare(
            "SELECT id, label, kind, fire_at, item_id, lead_s, hour, minute, weekdays,
                    last_fired_at, snoozed_until
             FROM reminder WHERE deleted_at IS NULL ORDER BY created_at",
        )?;
        let rows = st.query_map([], |r| {
            Ok(RawReminder {
                id: r.get(0)?,
                label: r.get(1)?,
                kind: r.get(2)?,
                fire_at: r.get(3)?,
                item_id: r.get(4)?,
                lead_s: r.get(5)?,
                hour: r.get(6)?,
                minute: r.get(7)?,
                weekdays: r.get(8)?,
                last_fired_at: r.get(9)?,
                snoozed_until: r.get(10)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            let raw = row?;
            let rule = match raw.kind.as_str() {
                "at" => ReminderRule::At(raw.fire_at.ok_or_else(|| missing("fire_at"))?),
                "before_item" => ReminderRule::BeforeItem {
                    item: parse_id(raw.item_id.as_deref().ok_or_else(|| missing("item_id"))?)?,
                    lead_s: raw.lead_s.unwrap_or(0),
                },
                "daily" => ReminderRule::Daily {
                    hour: u8::try_from(raw.hour.unwrap_or(0)).unwrap_or(0),
                    minute: u8::try_from(raw.minute.unwrap_or(0)).unwrap_or(0),
                    weekdays: u8::try_from(raw.weekdays.unwrap_or(127)).unwrap_or(127),
                },
                other => return Err(StoreError::Data(format!("rappel inconnu : {other}"))),
            };
            let reminder = Reminder {
                id: parse_id(&raw.id)?,
                rule,
                last_fired_at: raw.last_fired_at,
                snoozed_until: raw.snoozed_until,
            };
            out.push((reminder, raw.label));
        }
        Ok(out)
    }

    pub fn mark_reminder_fired(&self, id: Id, at: Ts) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE reminder SET last_fired_at = ?1, snoozed_until = NULL WHERE id = ?2",
            params![at, id.to_string()],
        )?;
        Ok(())
    }

    pub fn snooze_reminder(&self, id: Id, until: Ts) -> Result<(), StoreError> {
        self.conn.execute(
            "UPDATE reminder SET snoozed_until = ?1 WHERE id = ?2",
            params![until, id.to_string()],
        )?;
        Ok(())
    }

    pub fn delete_reminder(&self, id: Id, now: Ts) -> Result<bool, StoreError> {
        let n = self.conn.execute(
            "UPDATE reminder SET deleted_at = ?1 WHERE id = ?2 AND deleted_at IS NULL",
            params![now, id.to_string()],
        )?;
        Ok(n > 0)
    }

    // ---------- Réglages ----------

    pub fn get_setting(&self, name: &str) -> Result<Option<String>, StoreError> {
        Ok(self
            .conn
            .query_row(
                "SELECT value FROM app_setting WHERE name = ?1",
                params![name],
                |r| r.get(0),
            )
            .optional()?)
    }

    pub fn set_setting(&self, name: &str, value: &str) -> Result<(), StoreError> {
        self.conn.execute(
            "INSERT INTO app_setting (name, value) VALUES (?1, ?2)
             ON CONFLICT(name) DO UPDATE SET value = excluded.value",
            params![name, value],
        )?;
        Ok(())
    }

    /// Dernier instant où l'application a vérifié les rappels.
    pub fn last_run(&self) -> Result<Option<Ts>, StoreError> {
        Ok(self
            .get_setting("last_run")?
            .and_then(|v| v.parse::<i64>().ok()))
    }

    pub fn set_last_run(&self, ts: Ts) -> Result<(), StoreError> {
        self.set_setting("last_run", &ts.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use plano_core::schedule::weekday;

    #[test]
    fn cycle_plan_tache() {
        let s = Store::open(":memory:").unwrap();
        let plan = s.add_plan("Sport", 1).unwrap();
        let t = s.add_item(plan, "Séance jambes", 2).unwrap();
        assert_eq!(s.list_items(plan).unwrap().len(), 1);
        assert!(s.set_status(t, ItemStatus::Termine, 3).unwrap());
        assert_eq!(s.list_items(plan).unwrap()[0].status, ItemStatus::Termine);
        assert_eq!(s.list_plans().unwrap().len(), 1);
        assert_eq!(s.item_title(t).unwrap(), Some("Séance jambes".to_string()));
    }

    #[test]
    fn declencheurs_aller_retour() {
        let s = Store::open(":memory:").unwrap();
        let plan = s.add_plan("Sport", 1).unwrap();
        let seance = s.add_item(plan, "Séance", 2).unwrap();
        let depart = s.add_item(plan, "Départ", 2).unwrap();
        let libre = s.add_item(plan, "Sans horaire", 2).unwrap();

        let t1 = Trigger::Absolute(64_800);
        let t2 = Trigger::RelativeTo {
            anchor: seance,
            edge: Edge::Start,
            offset_s: -2_400,
        };
        assert!(s.set_trigger(seance, t1, 5_400, 3).unwrap());
        assert!(s.set_trigger(depart, t2, 0, 3).unwrap());
        assert!(s.set_trigger(libre, Trigger::Manual, 0, 3).unwrap());

        let items = s.list_schedule_items().unwrap();
        assert_eq!(items.len(), 2);
        let a = items.iter().find(|i| i.id == seance).unwrap();
        let b = items.iter().find(|i| i.id == depart).unwrap();
        assert_eq!(a.trigger, t1);
        assert_eq!(a.duration_s, 5_400);
        assert_eq!(b.trigger, t2);
    }

    #[test]
    fn rappels_aller_retour() {
        let s = Store::open(":memory:").unwrap();
        let item = Id::new();
        let r1 = s
            .add_reminder(ReminderRule::At(1_000), "Appeler", 1)
            .unwrap();
        let r2 = s
            .add_reminder(
                ReminderRule::Daily {
                    hour: 8,
                    minute: 30,
                    weekdays: weekday::WORKDAYS,
                },
                "Vitamines",
                2,
            )
            .unwrap();
        let r3 = s
            .add_reminder(
                ReminderRule::BeforeItem { item, lead_s: 900 },
                "Préparer le sac",
                3,
            )
            .unwrap();

        s.mark_reminder_fired(r1, 1_000).unwrap();
        s.snooze_reminder(r2, 5_000).unwrap();

        let list = s.list_reminders().unwrap();
        assert_eq!(list.len(), 3);
        assert_eq!(list[0].0.id, r1);
        assert_eq!(list[0].0.last_fired_at, Some(1_000));
        assert_eq!(list[0].1, "Appeler");
        assert_eq!(list[1].0.snoozed_until, Some(5_000));
        assert_eq!(
            list[1].0.rule,
            ReminderRule::Daily {
                hour: 8,
                minute: 30,
                weekdays: weekday::WORKDAYS
            }
        );
        assert_eq!(list[2].0.id, r3);
        assert_eq!(
            list[2].0.rule,
            ReminderRule::BeforeItem { item, lead_s: 900 }
        );

        assert!(s.delete_reminder(r1, 10).unwrap());
        assert_eq!(s.list_reminders().unwrap().len(), 2);
    }

    #[test]
    fn derniere_verification() {
        let s = Store::open(":memory:").unwrap();
        assert_eq!(s.last_run().unwrap(), None);
        s.set_last_run(1_234).unwrap();
        s.set_last_run(5_678).unwrap();
        assert_eq!(s.last_run().unwrap(), Some(5_678));
    }
}
