//! plano-store : persistance locale SQLite (mode WAL, migrations par user_version).

use plano_core::{Id, Item, ItemStatus, Ts};
use rusqlite::{params, Connection};

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

pub struct Store {
    conn: Connection,
}

fn parse_id(s: &str) -> Result<Id, StoreError> {
    s.parse::<Id>()
        .map_err(|e| StoreError::Data(format!("{e}")))
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
        Ok(())
    }

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

    pub fn set_status(&self, item_id: Id, status: ItemStatus, now: Ts) -> Result<bool, StoreError> {
        let n = self.conn.execute(
            "UPDATE item SET status = ?1, updated_at = ?2 WHERE id = ?3 AND deleted_at IS NULL",
            params![status.as_str(), now, item_id.to_string()],
        )?;
        Ok(n > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cycle_plan_tache() {
        let s = Store::open(":memory:").unwrap();
        let plan = s.add_plan("Sport", 1).unwrap();
        let t = s.add_item(plan, "Séance jambes", 2).unwrap();
        assert_eq!(s.list_items(plan).unwrap().len(), 1);
        assert!(s.set_status(t, ItemStatus::Termine, 3).unwrap());
        assert_eq!(s.list_items(plan).unwrap()[0].status, ItemStatus::Termine);
        assert_eq!(s.list_plans().unwrap().len(), 1);
    }
}
